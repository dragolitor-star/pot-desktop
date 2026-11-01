// Native PDF output assembly (Phase 2, HANDOFF section 8 #3).
//
// Assembles a translated PDF with `printpdf` instead of the webview print
// pipeline. Three modes:
//   - "translated":  flowed translated text only (no original).
//   - "bilingual":   per source page -> embedded rendered ORIGINAL page image,
//                    then the translated paragraphs flowed beneath it.
//   - "sidebyside":  per source page -> original image in the left column, the
//                    translated text in the right column.
//
// The translation is rendered as real, selectable text with an embedded font.
// A small Latin/Turkish/Cyrillic font is bundled; CJK targets lazily download a
// Noto Sans CJK font (mirrors `ensure_pdfium_library`).
//
// NOTE: real font embedding requires printpdf's `text_layout` feature (see
// Cargo.toml). Without it, `ParsedFont::from_bytes` is a stub with an empty glyph
// table, every external font is treated as "unused", dropped on save, and text
// renders blank. printpdf 0.9.x also has font subsetting hard-disabled internally,
// so the *full* font is embedded — fine for the ~740 KB Latin font, but CJK Noto
// fonts (~16 MB) make CJK output PDFs large. Acceptable for now (correctness first).

use pdfium_render::prelude::*;
use printpdf::*;
// `PdfDocument` and `PdfPage` exist in BOTH preludes; pin them to printpdf
// (explicit imports shadow the glob imports and resolve the ambiguity).
use printpdf::{PdfDocument, PdfPage};
use serde::Deserialize;
use std::path::PathBuf;

use crate::document::pdf::ensure_pdfium_library;

// Bundled font covering Latin + Turkish + Cyrillic + Greek (DejaVu Sans, ~740 KB).
const LATIN_FONT: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

// A4 geometry, in PDF points (1 mm = 2.834645 pt).
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const MARGIN: f32 = 39.685; // 14 mm
const FONT_SIZE: f32 = 10.5;
const LINE_HEIGHT: f32 = FONT_SIZE * 1.42;
const PARA_GAP: f32 = 6.0;
const IMG_GAP: f32 = 10.0;
// Render width for the embedded original-page images.
const IMG_TARGET_WIDTH: i32 = 1000;

#[derive(Deserialize)]
pub struct ExportParagraph {
    #[serde(default)]
    pub translated: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct ExportPage {
    pub paragraphs: Vec<ExportParagraph>,
}

// ── Font metrics (glyph-advance based) for our own line wrapping ─────────────

struct FontMetrics<'a> {
    face: ttf_parser::Face<'a>,
    upem: f32,
}

impl<'a> FontMetrics<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        let face =
            ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("Font parse error: {e:?}"))?;
        let upem = face.units_per_em() as f32;
        Ok(Self { face, upem })
    }

    fn char_w(&self, c: char, size: f32) -> f32 {
        let adv = self
            .face
            .glyph_index(c)
            .and_then(|g| self.face.glyph_hor_advance(g))
            .unwrap_or((self.upem * 0.5) as u16);
        adv as f32 / self.upem * size
    }

    fn text_w(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.char_w(c, size)).sum()
    }
}

// Hard-break an overlong token (also the natural path for space-less scripts
// like CJK, where the whole paragraph is one "word").
fn hard_break_word(
    m: &FontMetrics,
    word: &str,
    max_w: f32,
    size: f32,
    lines: &mut Vec<String>,
    line: &mut String,
    line_w: &mut f32,
) {
    for ch in word.chars() {
        let cw = m.char_w(ch, size);
        if *line_w + cw > max_w && !line.is_empty() {
            lines.push(std::mem::take(line));
            *line_w = 0.0;
        }
        line.push(ch);
        *line_w += cw;
    }
}

fn wrap_paragraph(m: &FontMetrics, text: &str, max_w: f32, size: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_w = 0.0f32;
    let space_w = m.char_w(' ', size);

    for word in text.split_whitespace() {
        let word_w = m.text_w(word, size);
        if line.is_empty() {
            if word_w <= max_w {
                line.push_str(word);
                line_w = word_w;
            } else {
                hard_break_word(m, word, max_w, size, &mut lines, &mut line, &mut line_w);
            }
        } else if line_w + space_w + word_w <= max_w {
            line.push(' ');
            line.push_str(word);
            line_w += space_w + word_w;
        } else {
            lines.push(std::mem::take(&mut line));
            line_w = 0.0;
            if word_w <= max_w {
                line.push_str(word);
                line_w = word_w;
            } else {
                hard_break_word(m, word, max_w, size, &mut lines, &mut line, &mut line_w);
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ── Page layout state machine ───────────────────────────────────────────────

struct Layout {
    font_id: FontId,
    pages: Vec<Vec<Op>>,
    cur: Vec<Op>,
    y: f32,
    in_text: bool,
    cur_color: (f32, f32, f32),
}

impl Layout {
    fn new(font_id: FontId) -> Self {
        Self {
            font_id,
            pages: Vec::new(),
            cur: Vec::new(),
            y: PAGE_H_PT - MARGIN,
            in_text: false,
            cur_color: (0.0, 0.0, 0.0),
        }
    }

    fn content_top(&self) -> f32 {
        PAGE_H_PT - MARGIN
    }
    fn content_bottom(&self) -> f32 {
        MARGIN
    }

    fn begin_text(&mut self) {
        if !self.in_text {
            self.cur.push(Op::StartTextSection);
            self.cur.push(Op::SetFont {
                font: PdfFontHandle::External(self.font_id.clone()),
                size: Pt(FONT_SIZE),
            });
            let (r, g, b) = self.cur_color;
            self.cur.push(Op::SetFillColor {
                col: Color::Rgb(Rgb::new(r, g, b, None)),
            });
            self.in_text = true;
        }
    }

    fn end_text(&mut self) {
        if self.in_text {
            self.cur.push(Op::EndTextSection);
            self.in_text = false;
        }
    }

    fn set_color(&mut self, c: (f32, f32, f32)) {
        if c != self.cur_color {
            self.cur_color = c;
            if self.in_text {
                self.cur.push(Op::SetFillColor {
                    col: Color::Rgb(Rgb::new(c.0, c.1, c.2, None)),
                });
            }
        }
    }

    fn new_page(&mut self) {
        self.end_text();
        if !self.cur.is_empty() {
            self.pages.push(std::mem::take(&mut self.cur));
        }
        self.y = self.content_top();
    }

    fn finish(&mut self) {
        self.end_text();
        if !self.cur.is_empty() {
            self.pages.push(std::mem::take(&mut self.cur));
        }
    }

    fn add_gap(&mut self, g: f32) {
        self.y -= g;
    }

    fn write_lines(&mut self, lines: &[String], x_left: f32, color: (f32, f32, f32)) {
        self.set_color(color);
        for line in lines {
            if self.y - LINE_HEIGHT < self.content_bottom() {
                self.new_page();
            }
            self.begin_text();
            // Absolute placement per line: `SetTextCursor` serializes to PDF `Td`,
            // which is RELATIVE to the previous line's origin, so absolute coords
            // would compound and throw every line after the first off-page. `Tm`
            // (TextMatrix::Translate) sets the text matrix absolutely.
            self.cur.push(Op::SetTextMatrix {
                matrix: TextMatrix::Translate(Pt(x_left), Pt(self.y)),
            });
            self.cur.push(Op::ShowText {
                items: vec![TextItem::Text(line.clone())],
            });
            self.y -= LINE_HEIGHT;
        }
    }

    fn place_image(
        &mut self,
        id: &XObjectId,
        w_px: u32,
        h_px: u32,
        x_left: f32,
        target_w: f32,
        max_h: f32,
    ) {
        if w_px == 0 || h_px == 0 {
            return;
        }
        self.end_text();
        let aspect = h_px as f32 / w_px as f32;
        let mut draw_w = target_w;
        let mut draw_h = draw_w * aspect;
        if draw_h > max_h {
            draw_h = max_h;
            draw_w = draw_h / aspect;
        }
        if self.y - draw_h < self.content_bottom() {
            self.new_page();
        }
        let bottom = self.y - draw_h;
        let dpi = w_px as f32 * 72.0 / draw_w;
        self.cur.push(Op::UseXobject {
            id: id.clone(),
            transform: XObjectTransform {
                translate_x: Some(Pt(x_left)),
                translate_y: Some(Pt(bottom)),
                dpi: Some(dpi),
                ..Default::default()
            },
        });
        self.y = bottom;
    }
}

fn write_paragraph(
    layout: &mut Layout,
    m: &FontMetrics,
    para: &ExportParagraph,
    x: f32,
    w: f32,
    normal: (f32, f32, f32),
    error_col: (f32, f32, f32),
) {
    let (text, color) = if !para.translated.trim().is_empty() {
        (para.translated.clone(), normal)
    } else if let Some(err) = &para.error {
        (format!("[Translation error: {err}]"), error_col)
    } else {
        ("[Not translated]".to_string(), error_col)
    };
    let lines = wrap_paragraph(m, &text, w, FONT_SIZE);
    if lines.is_empty() {
        return;
    }
    layout.write_lines(&lines, x, color);
    layout.add_gap(PARA_GAP);
}

// Render every source page to an embedded image XObject (id + pixel dims).
fn render_source_images(
    app_handle: &tauri::AppHandle,
    doc: &mut PdfDocument,
    path: &str,
) -> Result<Vec<(XObjectId, u32, u32)>, String> {
    let lib_path = ensure_pdfium_library(app_handle)?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );
    let pdoc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| e.to_string())?;
    let cfg = PdfRenderConfig::new().set_target_width(IMG_TARGET_WIDTH);

    let tmp_dir = std::env::temp_dir();
    let mut out = Vec::new();
    for (idx, page) in pdoc.pages().iter().enumerate() {
        let bitmap = page.render_with_config(&cfg).map_err(|e| e.to_string())?;
        let image = bitmap.as_image();
        let w = image.width();
        let h = image.height();
        // Encode to a temp PNG (DynamicImage::save infers PNG from the extension),
        // then hand the encoded bytes to printpdf. Using a file avoids depending on
        // the exact `image` crate version pdfium-render re-exports.
        let tmp = tmp_dir.join(format!("ct_export_{}_{}.png", std::process::id(), idx));
        image
            .save(&tmp)
            .map_err(|e| format!("Failed to encode page image: {e}"))?;
        let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&tmp);
        let raw = RawImage::decode_from_bytes(&bytes, &mut Vec::new())
            .map_err(|e| format!("Failed to decode page image: {e:?}"))?;
        let id = doc.add_image(&raw);
        out.push((id, w, h));
    }
    Ok(out)
}

fn is_cjk_target(lang: &str) -> bool {
    matches!(lang, "zh_cn" | "zh_tw" | "ja" | "ko" | "yue")
}

// Lazily fetch and cache a Noto Sans CJK font for the target, mirroring
// `ensure_pdfium_library`. Keeps the shipped binary small.
fn ensure_cjk_font(app_handle: &tauri::AppHandle, lang: &str) -> Result<PathBuf, String> {
    let (file_name, url) = match lang {
        "zh_cn" => (
            "NotoSansCJKsc-Regular.otf",
            "https://cdn.jsdelivr.net/gh/notofonts/noto-cjk@main/Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf",
        ),
        "zh_tw" | "yue" => (
            "NotoSansCJKtc-Regular.otf",
            "https://cdn.jsdelivr.net/gh/notofonts/noto-cjk@main/Sans/OTF/TraditionalChinese/NotoSansCJKtc-Regular.otf",
        ),
        "ja" => (
            "NotoSansCJKjp-Regular.otf",
            "https://cdn.jsdelivr.net/gh/notofonts/noto-cjk@main/Sans/OTF/Japanese/NotoSansCJKjp-Regular.otf",
        ),
        "ko" => (
            "NotoSansCJKkr-Regular.otf",
            "https://cdn.jsdelivr.net/gh/notofonts/noto-cjk@main/Sans/OTF/Korean/NotoSansCJKkr-Regular.otf",
        ),
        _ => return Err(format!("No CJK font mapping for {lang}")),
    };

    let config_dir = dirs::config_dir().ok_or("Could not find user config directory")?;
    let app_dir = config_dir
        .join(&app_handle.config().tauri.bundle.identifier)
        .join("fonts");
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    let font_path = app_dir.join(file_name);
    if font_path.exists() {
        return Ok(font_path);
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "Failed to download CJK font for '{}': HTTP {}",
            lang,
            resp.status()
        ));
    }
    let bytes = resp.bytes().map_err(|e| e.to_string())?;
    std::fs::write(&font_path, &bytes).map_err(|e| e.to_string())?;
    Ok(font_path)
}

/// Assemble a translated PDF natively and write it to `output_path`.
#[tauri::command]
pub fn export_translated_pdf(
    app_handle: tauri::AppHandle,
    source_pdf_path: String,
    output_path: String,
    mode: String,
    target_lang: String,
    pages: Vec<ExportPage>,
) -> Result<(), String> {
    let font_bytes: Vec<u8> = if is_cjk_target(&target_lang) {
        let p = ensure_cjk_font(&app_handle, &target_lang)?;
        std::fs::read(&p).map_err(|e| format!("Failed to read CJK font: {e}"))?
    } else {
        LATIN_FONT.to_vec()
    };

    let metrics = FontMetrics::new(&font_bytes)?;

    let parsed = ParsedFont::from_bytes(&font_bytes, 0, &mut Vec::new())
        .ok_or("Failed to parse embedded font for PDF output")?;
    let mut doc = PdfDocument::new("Translated Document");
    let font_id = doc.add_font(&parsed);

    let need_images = mode == "bilingual" || mode == "sidebyside";
    let images = if need_images {
        render_source_images(&app_handle, &mut doc, &source_pdf_path)?
    } else {
        Vec::new()
    };

    let mut layout = Layout::new(font_id);
    let normal = (0.0, 0.0, 0.0);
    let error_col = (0.62, 0.05, 0.10);

    match mode.as_str() {
        "translated" => {
            let content_w = PAGE_W_PT - 2.0 * MARGIN;
            for page in &pages {
                for para in &page.paragraphs {
                    write_paragraph(
                        &mut layout,
                        &metrics,
                        para,
                        MARGIN,
                        content_w,
                        normal,
                        error_col,
                    );
                }
            }
        }
        "bilingual" => {
            let content_w = PAGE_W_PT - 2.0 * MARGIN;
            let max_img_h = (PAGE_H_PT - 2.0 * MARGIN) * 0.62;
            for (i, page) in pages.iter().enumerate() {
                if i > 0 {
                    layout.new_page();
                }
                if let Some((id, w, h)) = images.get(i) {
                    layout.place_image(id, *w, *h, MARGIN, content_w, max_img_h);
                    layout.add_gap(IMG_GAP);
                }
                for para in &page.paragraphs {
                    write_paragraph(
                        &mut layout,
                        &metrics,
                        para,
                        MARGIN,
                        content_w,
                        normal,
                        error_col,
                    );
                }
            }
        }
        "sidebyside" => {
            let gutter = 14.0;
            let col_w = (PAGE_W_PT - 2.0 * MARGIN - gutter) / 2.0;
            let left_x = MARGIN;
            let right_x = MARGIN + col_w + gutter;
            let max_img_h = PAGE_H_PT - 2.0 * MARGIN;
            for (i, page) in pages.iter().enumerate() {
                if i > 0 {
                    layout.new_page();
                }
                let top = layout.content_top();
                if let Some((id, w, h)) = images.get(i) {
                    layout.place_image(id, *w, *h, left_x, col_w, max_img_h);
                }
                // Right column starts back at the top of the same page.
                layout.y = top;
                for para in &page.paragraphs {
                    write_paragraph(
                        &mut layout,
                        &metrics,
                        para,
                        right_x,
                        col_w,
                        normal,
                        error_col,
                    );
                }
            }
        }
        _ => return Err(format!("Unknown export mode: {mode}")),
    }

    layout.finish();

    if layout.pages.is_empty() {
        return Err("Nothing to export".to_string());
    }

    let pdf_pages: Vec<PdfPage> = layout
        .pages
        .into_iter()
        .map(|ops| PdfPage::new(Mm(210.0), Mm(297.0), ops))
        .collect();

    let save_opts = PdfSaveOptions {
        subset_fonts: true,
        ..Default::default()
    };
    let bytes = doc.with_pages(pdf_pages).save(&save_opts, &mut Vec::new());
    std::fs::write(&output_path, bytes).map_err(|e| format!("Failed to write PDF: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> FontMetrics<'static> {
        FontMetrics::new(LATIN_FONT).unwrap()
    }

    #[test]
    fn cjk_target_detection() {
        assert!(is_cjk_target("zh_cn"));
        assert!(is_cjk_target("ja"));
        assert!(is_cjk_target("ko"));
        assert!(!is_cjk_target("tr"));
        assert!(!is_cjk_target("en"));
    }

    #[test]
    fn wrap_breaks_long_text_into_multiple_lines() {
        let m = metrics();
        let text = "word ".repeat(80);
        let lines = wrap_paragraph(&m, text.trim(), 200.0, FONT_SIZE);
        assert!(lines.len() > 1, "expected wrapping into multiple lines");
        for l in &lines {
            assert!(
                m.text_w(l, FONT_SIZE) <= 200.0 + 0.01,
                "line exceeds max width: {l}"
            );
        }
    }

    #[test]
    fn wrap_hard_breaks_overlong_token() {
        let m = metrics();
        // A single token far wider than the line must still be split.
        let token = "A".repeat(500);
        let lines = wrap_paragraph(&m, &token, 100.0, FONT_SIZE);
        assert!(lines.len() > 1, "overlong token should hard-break");
    }

    #[test]
    fn wrap_empty_is_empty() {
        let m = metrics();
        assert!(wrap_paragraph(&m, "   ", 200.0, FONT_SIZE).is_empty());
    }
}
