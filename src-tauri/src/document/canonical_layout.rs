//! Object removal and checked fitting for canonical PDF blocks. No whiteout.
use super::*;
use crate::document::pdf::{BlockBox, PageLayout, TextAlign, TextBlock};
use std::collections::{HashMap, HashSet};

pub(crate) struct StyleFonts {
    pub regular: PdfFontToken,
    bold: PdfFontToken,
    italic: PdfFontToken,
    bold_italic: PdfFontToken,
    regular_bytes: Vec<u8>,
    bold_bytes: Vec<u8>,
    italic_bytes: Vec<u8>,
    bold_italic_bytes: Vec<u8>,
    fallback: PdfFontToken,
    fallback_bytes: Vec<u8>,
    condensed: PdfFontToken,
    condensed_bytes: Vec<u8>,
}
impl StyleFonts {
    pub(crate) fn load(doc: &mut PdfDocument) -> Result<Self, String> {
        fn bytes(name: &str, linux: &str) -> Vec<u8> {
            let win = std::env::var_os("WINDIR")
                .map(std::path::PathBuf::from)
                .map(|p| p.join("Fonts").join(name));
            win.and_then(|p| std::fs::read(p).ok())
                .or_else(|| std::fs::read(linux).ok())
                .unwrap_or_else(|| LATIN_FONT.to_vec())
        }
        // Installed fonts add no bundled asset/dependency. Every measured face
        // is exactly the face embedded in the PDF.
        let regular_bytes = LATIN_FONT.to_vec();
        let bold_bytes = bytes(
            "arialbd.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        );
        let italic_bytes = bytes(
            "ariali.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
        );
        let bold_italic_bytes = bytes(
            "arialbi.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
        );
        let fallback_bytes = bytes(
            "seguisym.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuMathTeXGyre.ttf",
        );
        let condensed_bytes = bytes(
            "ARIALN.TTF",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed.ttf",
        );
        let mut load = |b: &[u8]| {
            doc.fonts_mut()
                .load_true_type_from_bytes(b, true)
                .map_err(|e| e.to_string())
        };
        Ok(Self {
            regular: load(&regular_bytes)?,
            bold: load(&bold_bytes)?,
            italic: load(&italic_bytes)?,
            bold_italic: load(&bold_italic_bytes)?,
            fallback: load(&fallback_bytes)?,
            condensed: load(&condensed_bytes)?,
            regular_bytes,
            bold_bytes,
            italic_bytes,
            bold_italic_bytes,
            fallback_bytes,
            condensed_bytes,
        })
    }
    fn face(&self, bold: bool, italic: bool) -> (PdfFontToken, &[u8]) {
        match (bold, italic) {
            (true, true) => (self.bold_italic, &self.bold_italic_bytes),
            (true, false) => (self.bold, &self.bold_bytes),
            (false, true) => (self.italic, &self.italic_bytes),
            _ => (self.regular, &self.regular_bytes),
        }
    }
    fn edit_face(&self, edit: &BlockEdit) -> (PdfFontToken, &[u8]) {
        if edit.condensed {
            (self.condensed, &self.condensed_bytes)
        } else {
            self.face(edit.block.is_bold, edit.block.is_italic)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FittedText {
    pub size: f32,
    pub lines: Vec<String>,
    pub scale: f32,
    pub line_height: f32,
    pub ascent: f32,
    pub height: f32,
}

/// A fit is a proof that every line fits; failure is explicit, never clipping.
pub(crate) fn fit_complete(
    bytes: &[u8],
    text: &str,
    width: f32,
    height: f32,
    source_size: f32,
    single_line: bool,
) -> Result<FittedText, String> {
    fit_with_fallback(bytes, None, text, width, height, source_size, single_line)
}
fn fit_with_fallback(
    bytes: &[u8],
    fallback: Option<&[u8]>,
    text: &str,
    width: f32,
    height: f32,
    source_size: f32,
    single_line: bool,
) -> Result<FittedText, String> {
    let mut m = FontMetrics::new(bytes)?;
    m.fallback = fallback
        .map(|bytes| ttf_parser::Face::parse(bytes, 0).map_err(|e| e.to_string()))
        .transpose()?;
    if let Some(c) = text
        .chars()
        .find(|c| !c.is_whitespace() && !m.has_glyph(*c))
    {
        return Err(format!("Output font has no glyph for {c:?}"));
    }
    if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
        return Err("No free area for translated block".into());
    }
    // Small vector-diagram labels may already be below 5.5pt. Keep their
    // original size instead of enlarging them into adjacent arrows/labels.
    let floor = source_size.clamp(1.0, 5.5);
    let initial = source_size.clamp(floor, 48.0);
    let mut size = initial;
    loop {
        for scale in [1.0, 0.95, 0.9] {
            let lines = wrap_paragraph(&m, text, width / scale, size);
            let mut ascent = 0.0_f32;
            let mut descent = 0.0_f32;
            for c in text.chars().filter(|c| !c.is_whitespace()) {
                let face = if m.face.glyph_index(c).is_some() {
                    &m.face
                } else {
                    m.fallback.as_ref().unwrap_or(&m.face)
                };
                if let Some(b) = face.glyph_index(c).and_then(|g| face.glyph_bounding_box(g)) {
                    ascent = ascent.max(b.y_max as f32 / face.units_per_em() as f32 * size);
                    descent = descent.max(-(b.y_min as f32) / face.units_per_em() as f32 * size);
                }
            }
            if ascent == 0.0 {
                ascent = size * 0.8;
            }
            let line_height = size * 1.12;
            let needed = ascent + descent + lines.len().saturating_sub(1) as f32 * line_height;
            if needed <= height + 0.05
                && (!single_line || lines.len() <= 1)
                && lines
                    .iter()
                    .all(|l| m.text_w(l, size) * scale <= width + 0.05)
            {
                return Ok(FittedText {
                    size,
                    lines,
                    scale,
                    line_height,
                    ascent,
                    height: needed,
                });
            }
        }
        if size <= floor {
            return Err(format!(
                "Translation does not fit {width:.1}×{height:.1}pt at 5.5pt: {text}"
            ));
        }
        size = (size - 0.25).max(floor);
    }
}

#[derive(Clone)]
pub(crate) struct BlockEdit {
    block: TextBlock,
    fit: FittedText,
    rect: BlockBox,
    toc_page: Option<String>,
    condensed: bool,
}

/// For very small labels, retain a numbered marker in the original location
/// and place the complete translation in verified empty page space.
pub(crate) fn plan_with_callouts(
    page: &PdfPage,
    layout: &PageLayout,
    spec: &LayoutPage,
    fonts: &StyleFonts,
    target: &str,
) -> Result<Vec<BlockEdit>, String> {
    if !spec.block_ids.is_empty() && spec.block_ids.len() != spec.paragraphs.len() {
        return Err("Block ID/paragraph count mismatch; re-extract this PDF".into());
    }
    let mut edits = Vec::new();
    let mut pending = Vec::new();
    let mut seen = HashSet::new();
    for (i, paragraph) in spec.paragraphs.iter().enumerate() {
        let id = spec.block_ids.get(i).cloned();
        if let Some(id) = &id {
            if !seen.insert(id.clone()) { return Err(format!("Duplicate PDF block {id}")); }
        }
        let single = LayoutPage { paragraphs: vec![paragraph.clone()], block_ids: id.clone().into_iter().collect() };
        match plan(layout, &single, fonts, target) {
            Ok(mut result) => edits.append(&mut result),
            Err(error) if error.contains(": Translation does not fit ") => {
                let block = if let Some(id) = &id {
                    layout.blocks.iter().find(|b| &b.block_id == id)
                } else {
                    let mut matches = layout.blocks.iter().filter(|b| b.text.trim() == paragraph.original.trim());
                    let first = matches.next();
                    if matches.next().is_some() { None } else { first }
                }.ok_or_else(|| error.clone())?;
                if block.bbox.width() > 45.0 || block.bbox.height() > 24.0
                    || block.rotation.abs() > 1.0 || !block.form_text_refs.is_empty()
                    || block.toc_page.is_some() || block.protected {
                    return Err(error);
                }
                pending.push((block.clone(), paragraph.translated.trim().to_string(), error));
            }
            Err(error) => return Err(error),
        }
    }
    validate_collisions(&edits, fonts)?;
    if pending.is_empty() { return Ok(edits); }
    let canvas = EmptyPageSpace::new(page, layout)?;
    for (index, (block, translation, original_error)) in pending.into_iter().enumerate() {
        let label = format!("[{}]", index + 1);
        let (_, marker_font) = fonts.face(block.is_bold, block.is_italic);
        let marker_fit = fit_with_fallback(marker_font, Some(&fonts.fallback_bytes), &label,
            block.bbox.width(), block.bbox.height(), block.font_size, true)
            .map_err(|_| original_error.clone())?;
        let mut marker_block = block.clone();
        marker_block.alignment = TextAlign::Center;
        marker_block.first_line_indent = 0.0;
        let marker = BlockEdit { block: marker_block, fit: marker_fit, rect: block.bbox, toc_page: None, condensed: false };
        let note = format!("{label} {}", latin_punctuation(&translation));
        let (_, note_font) = fonts.face(false, false);
        let mut placed = None;
        for (rect, fit) in canvas.candidates(&block, &note, note_font, &fonts.fallback_bytes)? {
            let mut note_block = block.clone();
            note_block.bbox = rect;
            note_block.available_left = rect.left;
            note_block.available_right = rect.right;
            note_block.available_bottom = rect.bottom;
            note_block.alignment = TextAlign::Left;
            note_block.is_bold = false;
            note_block.is_italic = false;
            note_block.text_color = (35, 35, 35);
            note_block.text_alpha = 255;
            note_block.first_line_indent = 0.0;
            note_block.obj_indices.clear();
            note_block.form_text_refs.clear();
            let note_edit = BlockEdit { block: note_block, fit, rect, toc_page: None, condensed: false };
            let mut trial = edits.clone();
            trial.push(marker.clone());
            trial.push(note_edit.clone());
            if validate_collisions(&trial, fonts).is_ok() { placed = Some(note_edit); break; }
        }
        let note_edit = placed.ok_or_else(|| format!("{original_error}; no empty page space was found for a readable numbered note"))?;
        edits.push(marker);
        edits.push(note_edit);
    }
    Ok(edits)
}

struct EmptyPageSpace {
    width: f32,
    height: f32,
    pixels_w: usize,
    pixels_h: usize,
    dark_integral: Vec<u32>,
    obstacles: Vec<BlockBox>,
}
impl EmptyPageSpace {
    fn new(page: &PdfPage, layout: &PageLayout) -> Result<Self, String> {
        if !layout.width.is_finite() || !layout.height.is_finite()
            || layout.width <= 0.0 || layout.height <= 0.0
            || layout.width > 1500.0 || layout.height > 1500.0 {
            return Err("Page size is unsupported for automatic numbered notes".into());
        }
        let image = page.render_with_config(&PdfRenderConfig::new()
            .set_target_width((layout.width.ceil() as i32).min(1200))
            .set_maximum_height(1500))
            .map_err(|e| format!("Render page to locate empty space: {e}"))?.as_image().to_rgb8();
        let (w, h) = (image.width() as usize, image.height() as usize);
        if w == 0 || h == 0 { return Err("Cannot inspect empty page space".into()); }
        let mut integral = vec![0_u32; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0_u32;
            for x in 0..w {
                let p = image.get_pixel(x as u32, y as u32).0;
                row += u32::from(p[0] < 235 || p[1] < 235 || p[2] < 235);
                integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
            }
        }
        let mut obstacles = Vec::new();
        for object in page.objects().iter() {
            if object.as_text_object().is_some() { continue; }
            let Ok(bounds) = object.bounds() else { continue; };
            let b: BlockBox = bounds.into();
            let area = b.width() * b.height();
            // Page backgrounds and long thin rules are covered by pixel checks.
            if b.width() >= 12.0 && b.height() >= 12.0 && area < layout.width * layout.height * 0.7 {
                obstacles.push(b);
            }
        }
        Ok(Self { width: layout.width, height: layout.height, pixels_w: w, pixels_h: h, dark_integral: integral, obstacles })
    }
    fn empty(&self, rect: BlockBox) -> bool {
        if self.obstacles.iter().any(|o| rect.left < o.right + 2.0 && rect.right > o.left - 2.0 && rect.bottom < o.top + 2.0 && rect.top > o.bottom - 2.0) { return false; }
        let sx = self.pixels_w as f32 / self.width;
        let sy = self.pixels_h as f32 / self.height;
        let x0 = ((rect.left - 2.0) * sx).floor().max(0.0) as usize;
        let x1 = ((rect.right + 2.0) * sx).ceil().min(self.pixels_w as f32) as usize;
        let y0 = ((self.height - rect.top - 2.0) * sy).floor().max(0.0) as usize;
        let y1 = ((self.height - rect.bottom + 2.0) * sy).ceil().min(self.pixels_h as f32) as usize;
        let stride = self.pixels_w + 1;
        let sum = self.dark_integral[y1 * stride + x1] + self.dark_integral[y0 * stride + x0]
            - self.dark_integral[y0 * stride + x1] - self.dark_integral[y1 * stride + x0];
        sum == 0
    }
    fn candidates(&self, block: &TextBlock, text: &str, font: &[u8], fallback: &[u8]) -> Result<Vec<(BlockBox, FittedText)>, String> {
        let mut choices = Vec::new();
        for width in [110.0_f32, 150.0, 200.0] {
            for height in [14.0_f32, 24.0, 36.0] {
                let Ok(fit) = fit_with_fallback(font, Some(fallback), text, width, height, 8.0, false) else { continue; };
                if fit.size < 7.99 { continue; }
                let mut y = 10.0;
                while y + height <= self.height - 10.0 {
                    let mut x = 10.0;
                    while x + width <= self.width - 10.0 {
                        let rect = BlockBox { left: x, bottom: y, right: x + width, top: y + height };
                        if self.empty(rect) {
                            let dx = (x + width / 2.0) - (block.bbox.left + block.bbox.right) / 2.0;
                            let dy = (y + height / 2.0) - (block.bbox.bottom + block.bbox.top) / 2.0;
                            choices.push((dx * dx + dy * dy, rect, fit.clone()));
                        }
                        x += 5.0;
                    }
                    y += 5.0;
                }
            }
        }
        choices.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(choices.into_iter().map(|(_, rect, fit)| (rect, fit)).collect())
    }
}

pub(crate) fn form_removals(edits: &[BlockEdit]) -> Vec<crate::document::form_text::FormTextRef> {
    edits
        .iter()
        .flat_map(|e| e.block.form_text_refs.iter().cloned())
        .collect()
}

pub(crate) fn text_object_indices(edits: &[BlockEdit]) -> Result<HashSet<usize>, String> {
    if edits.iter().any(|e| !e.block.form_text_refs.is_empty()) {
        return Err("Pattern-preserving export does not yet support translated text inside vector Forms".into());
    }
    Ok(edits.iter().flat_map(|e| e.block.obj_indices.iter().copied()).collect())
}

fn latin_punctuation(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '：' => ':',
            '；' => ';',
            '，' | '、' => ',',
            '。' => '.',
            '！' => '!',
            '？' => '?',
            '（' => '(',
            '）' => ')',
            '【' | '［' => '[',
            '】' | '］' => ']',
            '　' => ' ',
            _ => c,
        })
        .collect()
}

pub(crate) fn plan(
    layout: &PageLayout,
    spec: &LayoutPage,
    fonts: &StyleFonts,
    target: &str,
) -> Result<Vec<BlockEdit>, String> {
    if !spec.block_ids.is_empty() && spec.block_ids.len() != spec.paragraphs.len() {
        return Err("Block ID/paragraph count mismatch; re-extract this PDF".into());
    }
    let by_id: HashMap<_, _> = layout
        .blocks
        .iter()
        .map(|b| (b.block_id.as_str(), b))
        .collect();
    let mut claimed = HashSet::new();
    let mut edits = Vec::new();
    for (i, p) in spec.paragraphs.iter().enumerate() {
        let block = if let Some(id) = spec.block_ids.get(i) {
            *by_id
                .get(id.as_str())
                .ok_or_else(|| format!("Unknown PDF block {id}; re-extract this PDF"))?
        } else {
            // Compatibility for older clients, only when the match is unique.
            let matches: Vec<_> = layout
                .blocks
                .iter()
                .filter(|b| b.text.trim() == p.original.trim())
                .collect();
            if matches.len() != 1 {
                return Err(format!(
                    "Ambiguous legacy PDF paragraph on page {}; re-extract this PDF",
                    layout.page_index + 1
                ));
            }
            matches[0]
        };
        if block.text.trim() != p.original.trim() {
            return Err(format!(
                "Source changed for {}; re-extract this PDF",
                block.block_id
            ));
        }
        if !claimed.insert(&block.block_id) {
            return Err(format!("Duplicate PDF block {}", block.block_id));
        }
        if block.protected {
            continue;
        }
        let mut translation = p.translated.trim().replace(['\u{f09f}', '\u{f06c}'], "•");
        if crate::document::translate::residual_cjk("中", target) {
            translation = latin_punctuation(&translation);
        }
        if translation.is_empty() || translation.starts_with("__ERROR__") {
            return Err(format!("Untranslated block {}", block.block_id));
        }
        if crate::document::translate::residual_cjk(&translation, target) {
            return Err(format!("Residual CJK in {}", block.block_id));
        }
        if crate::document::translate::protected_token_loss(&block.text, &translation) {
            return Err(format!(
                "Technical token missing in {}; translate this block again",
                block.block_id
            ));
        }
        if translation == block.text.trim() {
            continue;
        }
        let (_, bytes) = fonts.face(block.is_bold, block.is_italic);
        let mut rect = block.bbox;
        let diagram_label = block.form_bounds.is_some();
        let single_line_label = diagram_label && block.bbox.height() < block.font_size * 1.6;
        if diagram_label {
            rect.left = block.available_left.min(rect.left);
        }
        rect.right = block.available_right.max(rect.right);
        if block.alignment == TextAlign::Center {
            if diagram_label {
                let center = (block.bbox.left + block.bbox.right) / 2.0;
                let half = (center - rect.left)
                    .min(rect.right - center)
                    .max(block.bbox.width() / 2.0);
                rect.left = center - half;
                rect.right = center + half;
            } else {
                rect.left = block.bbox.left + block.bbox.right - rect.right;
            }
        }
        if !diagram_label {
            rect.bottom = block.available_bottom.min(rect.bottom);
        }
        let toc_page = block.toc_page.clone();
        if toc_page.is_some() {
            if let Some((title, _)) = crate::document::pdf::split_toc(&translation) {
                translation = title;
            }
            rect.right = layout.width - 55.5;
        }
        let mut fit_width = rect.width();
        if toc_page.is_some() {
            fit_width -= 38.0;
        }
        // Rotated watermark runs keep their original angle and baseline span.
        if block.rotation.abs() > 1.0 {
            fit_width = rect.width().hypot(rect.height());
        }
        let height = if block.rotation.abs() > 1.0 {
            block.font_size * 1.4
        } else {
            rect.height()
        };
        let regular_fit = fit_with_fallback(
            bytes,
            Some(&fonts.fallback_bytes),
            &translation,
            fit_width - block.first_line_indent,
            height,
            block.font_size,
            toc_page.is_some() || single_line_label,
        );
        // A real condensed face keeps cramped vector labels readable without
        // crossing arrows or reducing them below the 5.5pt floor.
        let condensed = regular_fit.is_err()
            && !block.form_text_refs.is_empty()
            && !block.is_bold
            && !block.is_italic;
        let fit = if condensed {
            fit_with_fallback(
                &fonts.condensed_bytes,
                Some(&fonts.fallback_bytes),
                &translation,
                fit_width - block.first_line_indent,
                height,
                block.font_size,
                toc_page.is_some() || single_line_label,
            )
        } else {
            regular_fit
        }
        .map_err(|e| format!("{}: {e}", block.block_id))?;
        edits.push(BlockEdit {
            block: block.clone(),
            fit,
            rect,
            toc_page,
            condensed,
        });
    }
    validate_collisions(&edits, fonts)?;
    Ok(edits)
}

pub(crate) fn validate_collisions(edits: &[BlockEdit], fonts: &StyleFonts) -> Result<(), String> {
    let mut occupied = Vec::new();
    for edit in edits.iter().filter(|e| e.block.rotation.abs() < 1.0) {
        let (_, bytes) = fonts.edit_face(edit);
        let mut metrics = FontMetrics::new(bytes)?;
        metrics.fallback =
            Some(ttf_parser::Face::parse(&fonts.fallback_bytes, 0).map_err(|e| e.to_string())?);
        let width = edit
            .fit
            .lines
            .iter()
            .map(|l| metrics.text_w(l, edit.fit.size) * edit.fit.scale)
            .fold(0.0_f32, f32::max);
        let left = match edit.block.alignment {
            TextAlign::Center => (edit.rect.left + edit.rect.right - width) / 2.0,
            TextAlign::Right => edit.rect.right - width,
            _ => edit.rect.left,
        };
        let right = if edit.toc_page.is_some() {
            edit.rect.right
        } else {
            left + width + edit.block.first_line_indent
        };
        let bounds = BlockBox {
            left,
            bottom: edit.rect.top - edit.fit.height,
            right,
            top: edit.rect.top,
        };
        for (id, other) in &occupied {
            let other: &BlockBox = other;
            if bounds.left < other.right - 0.3
                && bounds.right > other.left + 0.3
                && bounds.bottom < other.top - 0.3
                && bounds.top > other.bottom + 0.3
            {
                return Err(format!(
                    "Translated blocks {} and {id} would overlap; no PDF was written",
                    edit.block.block_id
                ));
            }
        }
        occupied.push((edit.block.block_id.as_str(), bounds));
    }
    Ok(())
}

pub(crate) fn apply(
    page: &mut PdfPage,
    edits: &[BlockEdit],
    fonts: &StyleFonts,
) -> Result<(), String> {
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let mut indices: Vec<_> = edits
        .iter()
        .flat_map(|e| e.block.obj_indices.iter().copied())
        .collect();
    indices.sort_unstable();
    if indices.windows(2).any(|w| w[0] == w[1]) {
        return Err("Text object belongs to multiple translated blocks".into());
    }
    for i in indices.into_iter().rev() {
        let objects = page.objects_mut();
        if objects
            .get(i)
            .map_err(|e| e.to_string())?
            .as_text_object()
            .is_none()
        {
            return Err(format!("Object {i} is no longer text"));
        }
        let removed = objects
            .remove_object_at_index(i)
            .map_err(|e| format!("Remove text object {i}: {e}"))?;
        destroy_removed_text(removed)?;
    }
    draw_edits(page, edits, fonts)?;
    page.regenerate_content()
        .map_err(|e| format!("Generate translated page content: {e}"))
}

pub(crate) fn draw_edits(page: &mut PdfPage, edits: &[BlockEdit], fonts: &StyleFonts) -> Result<(), String> {
    for edit in edits {
        let (font, bytes) = fonts.edit_face(edit);
        let mut m = FontMetrics::new(bytes)?;
        m.fallback =
            Some(ttf_parser::Face::parse(&fonts.fallback_bytes, 0).map_err(|e| e.to_string())?);
        let color = PdfColor::new(
            edit.block.text_color.0,
            edit.block.text_color.1,
            edit.block.text_color.2,
            edit.block.text_alpha,
        );
        for (k, line) in edit.fit.lines.iter().enumerate() {
            let width = m.text_w(line, edit.fit.size) * edit.fit.scale;
            let x = if edit.block.rotation.abs() > 1.0 {
                edit.block.baseline.0
            } else {
                match edit.block.alignment {
                    TextAlign::Center => (edit.rect.left + edit.rect.right - width) / 2.0,
                    TextAlign::Right => edit.rect.right - width,
                    _ => {
                        edit.rect.left
                            + if k == 0 {
                                edit.block.first_line_indent
                            } else {
                                0.0
                            }
                    }
                }
            };
            let y = if edit.block.rotation.abs() > 1.0 {
                edit.block.baseline.1
            } else {
                edit.rect.top - edit.fit.ascent - k as f32 * edit.fit.line_height
            };
            let angle = edit.block.rotation.to_radians();
            let mut runs: Vec<(bool, String)> = Vec::new();
            for c in line.chars() {
                let fallback = m.face.glyph_index(c).is_none();
                if let Some(last) = runs.last_mut().filter(|r| r.0 == fallback) {
                    last.1.push(c);
                } else {
                    runs.push((fallback, c.to_string()));
                }
            }
            let mut advance = 0.0;
            for (fallback, text) in runs {
                let run_x = x + advance * angle.cos();
                let run_y = y + advance * angle.sin();
                let mut object = page
                    .objects_mut()
                    .create_text_object(
                        PdfPoints::new(run_x),
                        PdfPoints::new(run_y),
                        crate::document::unicode_text::transport(&text),
                        if fallback { fonts.fallback } else { font },
                        PdfPoints::new(edit.fit.size),
                    )
                    .map_err(|e| e.to_string())?;
                object.set_fill_color(color).map_err(|e| e.to_string())?;
                object
                    .reset_matrix(PdfMatrix::new(
                        edit.fit.scale * angle.cos(),
                        edit.fit.scale * angle.sin(),
                        -angle.sin(),
                        angle.cos(),
                        run_x,
                        run_y,
                    ))
                    .map_err(|e| e.to_string())?;
                advance += m.text_w(&text, edit.fit.size) * edit.fit.scale;
            }
        }
        if let Some(number) = &edit.toc_page {
            let y = edit.rect.top - edit.fit.ascent;
            let right = edit.rect.right;
            let page_width = m.text_w(number, edit.fit.size);
            let title_width = m.text_w(&edit.fit.lines.join(" "), edit.fit.size) * edit.fit.scale;
            let leader_left = edit.rect.left + title_width + 6.0;
            let leader_right = right - page_width - 6.0;
            let dot_width = m.text_w(". ", edit.fit.size);
            let count = ((leader_right - leader_left) / dot_width).floor().max(0.0) as usize;
            for (x, text) in [
                (right - page_width, number.clone()),
                (leader_left, ". ".repeat(count)),
            ] {
                if text.is_empty() {
                    continue;
                }
                let mut object = page
                    .objects_mut()
                    .create_text_object(
                        PdfPoints::new(x),
                        PdfPoints::new(y),
                        &text,
                        font,
                        PdfPoints::new(edit.fit.size),
                    )
                    .map_err(|e| e.to_string())?;
                object.set_fill_color(color).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

fn destroy_removed_text(removed: PdfPageObject<'_>) -> Result<(), String> {
    // pdfium-render 0.8.37 implements Drop on BOTH PdfPageObject and its
    // PdfPageTextObject variant. After removal ownership is Unowned, so normal
    // enum drop calls FPDFPageObj_Destroy twice (native CHECK / illegal instruction).
    // Suppress the enum destructor and run the inner text destructor exactly once.
    let mut removed = std::mem::ManuallyDrop::new(removed);
    let text = removed
        .as_text_object_mut()
        .ok_or("Removed object was not text")?;
    // SAFETY: this object was detached successfully, this is its sole owning
    // wrapper, and ManuallyDrop prevents either destructor running again. The
    // storage is never read after this call. PDFium releases the native object.
    unsafe {
        std::ptr::drop_in_place(text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_single_line_never_discards_its_tail() {
        let text =
            "Lazerin montaj seviyesi düşüktür, lazerin eğimi ayarlanarak bu durum önlenebilir.";
        let fit = fit_complete(LATIN_FONT, text, 462.0, 16.4, 10.56, false).unwrap();
        assert_eq!(fit.lines.join(" "), text);
        assert!(fit.size >= 5.5);
    }
    #[test]
    fn expanded_cover_header_keeps_the_complete_translation() {
        let text = "Çin Dili ve Geleneksel Kültürü Görsel İllüstrasyon • Çince Karakterler ve Mürekkep Sanatına İlişkin Özel Konular";
        assert!(fit_complete(LATIN_FONT, text, 249.5, 10.0, 9.495, false).is_err());
        let fit = fit_complete(LATIN_FONT, text, 488.0, 14.0, 9.495, false).unwrap();
        assert_eq!(fit.lines.join(" "), text);
    }
    #[test]
    fn note_space_rejects_ink_and_graphic_obstacles() {
        let mut canvas = EmptyPageSpace {
            width: 20.0, height: 20.0, pixels_w: 20, pixels_h: 20,
            dark_integral: vec![0; 21 * 21], obstacles: vec![],
        };
        let rect = BlockBox { left: 5.0, bottom: 5.0, right: 10.0, top: 10.0 };
        assert!(canvas.empty(rect));
        canvas.obstacles.push(BlockBox { left: 6.0, bottom: 6.0, right: 8.0, top: 8.0 });
        assert!(!canvas.empty(rect));
        canvas.obstacles.clear();
        // Integral image of one dark pixel at image coordinate (6, 12).
        for y in 13..=20 { for x in 7..=20 { canvas.dark_integral[y * 21 + x] = 1; } }
        assert!(!canvas.empty(rect));
    }
    #[test]
    #[ignore = "Requires CTRANSLATE_TEST_PDF and CTRANSLATE_PDFIUM"]
    fn tiny_seal_has_a_readable_numbered_note() {
        let path = std::env::var("CTRANSLATE_TEST_PDF").unwrap();
        let pdfium = Pdfium::new(Pdfium::bind_to_library(std::env::var("CTRANSLATE_PDFIUM").unwrap()).unwrap());
        let mut doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        let fonts = StyleFonts::load(&mut doc).unwrap();
        let mut page = doc.pages().get(0).unwrap();
        let layout = crate::document::pdf::canonical_page_layout(&page, 0).unwrap();
        let block = layout.blocks.iter().find(|b| b.text.chars().filter(|c| !c.is_whitespace()).collect::<String>() == "墨韵").unwrap();
        let spec = LayoutPage { block_ids: vec![block.block_id.clone()], paragraphs: vec![LayoutParagraph {
            original: block.text.clone(), translated: "Mürekkebin Ritmi".into(),
        }] };
        let edits = plan_with_callouts(&page, &layout, &spec, &fonts, "tr").unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].fit.lines.join(" "), "[1]");
        assert_eq!(edits[1].fit.lines.join(" "), "[1] Mürekkebin Ritmi");
        assert!(edits[1].fit.size >= 8.0);
        assert!(edits[1].block.obj_indices.is_empty());
        validate_collisions(&edits, &fonts).unwrap();
        apply(&mut page, &edits, &fonts).unwrap();
    }
    #[test]
    fn impossible_fit_is_an_error_instead_of_a_partial_sentence() {
        assert!(fit_complete(LATIN_FONT, &"kelime ".repeat(100), 12.0, 12.0, 10.0, false).is_err());
    }
    #[test]
    fn missing_glyph_is_reported_instead_of_filtered_out() {
        assert!(fit_complete(LATIN_FONT, "步骤", 200.0, 30.0, 10.0, false).is_err());
    }

    #[test]
    fn translated_fullwidth_punctuation_remains_readable() {
        let text = latin_punctuation("【Lazer】：all_calib_data.json（x，y）；tamam。");
        let fit = fit_complete(LATIN_FONT, &text, 300.0, 24.0, 10.0, false).unwrap();
        assert_eq!(
            fit.lines.join(" "),
            "[Lazer]:all_calib_data.json(x,y);tamam."
        );
    }

    #[test]
    #[ignore = "Explicitly authorized Gemini verification only; requires CTRANSLATE_ALLOW_GEMINI=1"]
    fn render_manual_live_translation() {
        assert_eq!(
            std::env::var("CTRANSLATE_ALLOW_GEMINI").as_deref(),
            Ok("1"),
            "Live provider transfer must be explicitly authorized"
        );
        let directory = std::path::PathBuf::from(std::env::var("CTRANSLATE_AUDIT_DIR").unwrap());
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("layout-request.json")).unwrap())
                .unwrap();
        let request: LayoutRequest = serde_json::from_value(raw.clone()).unwrap();
        let selected: HashSet<usize> = raw["verificationPages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize - 1)
            .collect();
        let provider: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.join("verification-provider.json")).unwrap(),
        )
        .unwrap();
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::PathBuf::from(std::env::var("APPDATA").unwrap())
                    .join("com.pot-app.desktop/config.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let config = &settings[provider
            .get("visionInstance")
            .unwrap_or(&provider["instance"])
            .as_str()
            .unwrap()];
        let base = provider
            .get("visionBase")
            .unwrap_or(&provider["base"])
            .as_str()
            .unwrap();
        assert!(
            base.starts_with("https://generativelanguage.googleapis.com/"),
            "Only the authorized Google Gemini endpoint is allowed"
        );
        let glossary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("image-glossary.json")).unwrap())
                .unwrap();
        let vision_rpm = std::env::var("CTRANSLATE_VISION_RPM")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(4);
        let options: crate::document::image_translate::ImageTranslateOptions =
            serde_json::from_value(serde_json::json!({
            "apiKey":config["apiKey"],"apiBase":base,"model":provider.get("visionModel").unwrap_or(&provider["model"]),
            "sourceLang":"zh_cn","targetLang":"tr","rpmLimit":vision_rpm,"glossary":glossary
            }))
            .unwrap();
        let mut translator =
            crate::document::image_translate::ImageTranslator::new(&options).unwrap();
        translator
            .use_fixture_cache(&directory.join("live-images"))
            .unwrap();
        let pdfium = Pdfium::new(
            Pdfium::bind_to_library(std::env::var("CTRANSLATE_PDFIUM").unwrap()).unwrap(),
        );
        let private_source = crate::document::image_resources::PrivateImageSource::create(
            std::path::Path::new(&request.source_pdf_path),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        )
        .unwrap();
        let mut doc = pdfium
            .load_pdf_from_file(&private_source.path(), None)
            .unwrap();
        assert_eq!(request.pages.len(), doc.pages().len() as usize);
        let fonts = StyleFonts::load(&mut doc).unwrap();
        let mut plans = Vec::new();
        let mut failures = Vec::new();
        let mut fitted = Vec::new();
        for (page_index, spec) in request.pages.iter().enumerate() {
            if !selected.contains(&page_index) {
                plans.push(Vec::new());
                continue;
            }
            let page = doc.pages().get(page_index as u16).unwrap();
            let layout = crate::document::pdf::canonical_page_layout(&page, page_index).unwrap();
            assert_eq!(
                spec.paragraphs.len(),
                layout.blocks.len(),
                "Incomplete page {}",
                page_index + 1
            );
            match plan(&layout, spec, &fonts, "tr") {
                Ok(edits) => {
                    for edit in &edits {
                        fitted.push(serde_json::json!({"blockId":edit.block.block_id,"size":edit.fit.size,"lineCount":edit.fit.lines.len(),"scale":edit.fit.scale,"lines":edit.fit.lines,"bbox":edit.rect}));
                    }
                    plans.push(edits);
                }
                Err(error) => {
                    failures.push(error);
                    plans.push(Vec::new());
                }
            }
        }
        std::fs::write(
            directory.join("live-fit-report.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"fitted":fitted,"failures":failures}))
                .unwrap(),
        )
        .unwrap();
        assert!(failures.is_empty(), "Unfitted translations: {failures:#?}");
        let mut forms = Vec::new();
        let mut image_failures = Vec::new();
        for (page_index, edits) in plans.iter().enumerate() {
            if !selected.contains(&page_index) {
                continue;
            }
            eprintln!(
                "Live verification page {}: {} translated text blocks",
                page_index + 1,
                edits.len()
            );
            let mut page = doc.pages().get(page_index as u16).unwrap();
            let result = translator.translate_page(
                None,
                &pdfium,
                &doc,
                &mut page,
                || false,
                |message| eprintln!("Page {}: {message}", page_index + 1),
            );
            std::fs::write(directory.join("live-image-progress.json"),serde_json::to_vec_pretty(&serde_json::json!({"page":page_index+1,"processed":translator.processed,"cacheHits":translator.cache_hits,"error":result.as_ref().err()})).unwrap()).unwrap();
            if let Err(error) = &result {
                if error.starts_with("Local image OCR could not locate")
                    || error.starts_with("Image label ")
                    || error.starts_with("Vision returned overlapping text regions")
                {
                    image_failures.push(serde_json::json!({"page":page_index+1,"error":error}));
                    eprintln!("Recorded image review failure on page {}; continuing private full-document audit",page_index+1);
                    std::fs::write(
                        directory.join("live-image-failures.json"),
                        serde_json::to_vec_pretty(&image_failures).unwrap(),
                    )
                    .unwrap();
                    continue;
                }
            }
            result.unwrap();
            forms.extend(form_removals(edits));
            apply(&mut page, edits, &fonts).unwrap();
        }
        std::fs::write(
            directory.join("live-image-failures.json"),
            serde_json::to_vec_pretty(&image_failures).unwrap(),
        )
        .unwrap();
        assert!(
            image_failures.is_empty(),
            "Image review failures remain; no partial PDF is published: {image_failures:#?}"
        );
        let output = std::path::PathBuf::from(
            std::env::var("CTRANSLATE_VERIFIED_OUTPUT").unwrap_or(request.output_path),
        );
        assert_ne!(output, std::path::PathBuf::from(request.source_pdf_path));
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        let temporary = output.with_extension("verification.tmp");
        doc.save_to_file(&temporary).unwrap();
        crate::document::form_text::remove_translated_form_text(&temporary, &forms).unwrap();
        std::fs::rename(&temporary, &output).unwrap();
        eprintln!(
            "Live verification saved {} pages; images processed {}, reused {}",
            selected.len(),
            translator.processed,
            translator.cache_hits
        );
    }

    #[test]
    #[ignore = "Requires CTRANSLATE_LAYOUT_REQUEST and CTRANSLATE_PDFIUM; writes a diagnostic PDF"]
    fn render_manual_cached_diagnostic() {
        let request_path = std::env::var("CTRANSLATE_LAYOUT_REQUEST").expect("layout request path");
        let request: LayoutRequest =
            serde_json::from_slice(&std::fs::read(request_path).unwrap()).unwrap();
        let pdfium = Pdfium::new(
            Pdfium::bind_to_library(std::env::var("CTRANSLATE_PDFIUM").unwrap()).unwrap(),
        );
        let mut doc = pdfium
            .load_pdf_from_file(&request.source_pdf_path, None)
            .unwrap();
        eprintln!("Diagnostic: loading fonts");
        let fonts = StyleFonts::load(&mut doc).unwrap();
        let mut failures = Vec::new();
        let mut fitted = Vec::new();
        let mut forms = Vec::new();
        for (page_index, spec) in request.pages.iter().enumerate() {
            if spec.paragraphs.is_empty() {
                continue;
            }
            let mut page = doc.pages().get(page_index as u16).unwrap();
            eprintln!("Diagnostic: planning page {}", page_index + 1);
            let layout = crate::document::pdf::canonical_page_layout(&page, page_index).unwrap();
            let mut edits = Vec::new();
            for (i, paragraph) in spec.paragraphs.iter().enumerate() {
                let single = LayoutPage {
                    paragraphs: vec![paragraph.clone()],
                    block_ids: vec![spec.block_ids[i].clone()],
                };
                match plan(&layout, &single, &fonts, "tr") {
                    Ok(mut planned) => {
                        for edit in &planned {
                            fitted.push(serde_json::json!({"blockId":edit.block.block_id,"size":edit.fit.size,"lineCount":edit.fit.lines.len(),"scale":edit.fit.scale,"lines":edit.fit.lines,"bbox":edit.rect}));
                        }
                        edits.append(&mut planned);
                    }
                    Err(e) => failures.push(e),
                }
            }
            eprintln!(
                "Diagnostic: applying {} blocks on page {}",
                edits.len(),
                page_index + 1
            );
            if let Err(e) = validate_collisions(&edits, &fonts) {
                failures.push(e);
            }
            forms.extend(form_removals(&edits));
            apply(&mut page, &edits, &fonts).unwrap();
            eprintln!("Diagnostic: finished page {}", page_index + 1);
        }
        eprintln!("Diagnostic: saving PDF");
        doc.save_to_file(&request.output_path).unwrap();
        crate::document::form_text::remove_translated_form_text(
            std::path::Path::new(&request.output_path),
            &forms,
        )
        .unwrap();
        let report = std::path::Path::new(&request.output_path).with_extension("json");
        std::fs::write(
            report,
            serde_json::to_vec_pretty(&serde_json::json!({"fitted":fitted,"failures":failures}))
                .unwrap(),
        )
        .unwrap();
        assert!(failures.is_empty(), "Unfitted translations: {failures:#?}");
    }
}
