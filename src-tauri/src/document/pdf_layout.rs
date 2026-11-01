// Layout-preserving translated PDF export. Extraction and export share the
// canonical PageLayout, stable block IDs and exact source-object ownership.
// Translated text objects are removed and regenerated without opaque patches.
// Fitting either retains every translated word or reports a specific block.
// Optional Gemini Vision translation updates raster XObjects in place.
//
// Runs on a background thread and streams progress through Tauri events (all
// carry `runId` so a stale/superseded run is ignored by the window):
//   layout_export://progress { runId, percent, text }
//   layout_export://done     { runId, status: "completed"|"cancelled"|"error", message }

use log::{info, warn};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::document::image_service::ImageReplacement;
use crate::document::image_text::ImageOverlay;
use crate::document::pdf::ensure_pdfium_library;

#[path = "canonical_layout.rs"]
pub(crate) mod canonical;

// Bundled Latin/Turkish/Cyrillic font (same asset the native export embeds).
// CJK *target* output is intentionally out of scope for this in-place path (the
// reflow `export_translated_pdf` already covers CJK via downloaded Noto fonts);
// drawn text uses this TrueType face which pdfium can embed reliably.
const LATIN_FONT: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

const EVT_PROGRESS: &str = "layout_export://progress";
const EVT_DONE: &str = "layout_export://done";

// Only one assembly runs at a time; a newer run or an explicit cancel makes the
// older thread stop at the next page checkpoint.
static ACTIVE_RUN: AtomicU64 = AtomicU64::new(0);
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutParagraph {
    #[serde(default)]
    pub original: String,
    #[serde(default)]
    pub translated: String,
}

#[derive(Debug, Deserialize)]
pub struct LayoutPage {
    #[serde(default)]
    pub paragraphs: Vec<LayoutParagraph>,
    #[serde(default, rename = "blockIds")]
    pub block_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutRequest {
    pub run_id: u64,
    pub source_pdf_path: String,
    pub output_path: String,
    pub pages: Vec<LayoutPage>,
    #[serde(default)]
    pub target_lang: String,
    #[serde(default)]
    pub image_translation: Option<super::image_translate::ImageTranslateOptions>,
    // Legacy request field. Nonempty whole-image overlays are rejected because
    // they cover graphics; native Vision edits glyph pixels instead.
    #[serde(default)]
    pub image_overlays: Vec<ImageOverlay>,
    // Optional service results replace the existing image bitmap in place.
    #[serde(default)]
    pub image_replacements: Vec<ImageReplacement>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    run_id: u64,
    percent: f32,
    text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DoneEvent {
    run_id: u64,
    status: String,
    message: String,
}

/// Start the background layout-preserving assembly and return immediately.
/// Progress/result are delivered through the events documented above.
#[tauri::command]
pub fn export_layout_pdf(app_handle: tauri::AppHandle, request: LayoutRequest) {
    let run_id = request.run_id;
    ACTIVE_RUN.store(run_id, Ordering::SeqCst);
    CANCEL_FLAG.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        // A panic here would unwind the thread WITHOUT emitting `done`, leaving
        // the window on its last progress message with no way to tell that the
        // export died. Report it as an error instead.
        let app = app_handle.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            run_layout_export(app_handle, request);
        }));
        if let Err(payload) = result {
            let detail = panic_message(&payload);
            warn!("Layout PDF export panicked: {detail}");
            emit_done(
                &app,
                run_id,
                "error",
                &format!("Export failed unexpectedly: {detail}"),
            );
        }
    });
}

/// Request cancellation of the currently-running layout export.
#[tauri::command]
pub fn cancel_layout_export() {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn should_stop(run_id: u64) -> bool {
    CANCEL_FLAG.load(Ordering::SeqCst) || ACTIVE_RUN.load(Ordering::SeqCst) != run_id
}

fn emit_progress(app: &tauri::AppHandle, run_id: u64, percent: f32, text: &str) {
    use tauri::Manager;
    let payload = ProgressEvent {
            run_id,
            percent,
            text: text.to_string(),
        };
    if let Ok(json) = serde_json::to_string(&payload) { app.trigger_global(EVT_PROGRESS, Some(json)); }
    let _ = app.emit_all(EVT_PROGRESS, payload);
}

fn emit_done(app: &tauri::AppHandle, run_id: u64, status: &str, message: &str) {
    use tauri::Manager;
    let payload = DoneEvent {
            run_id,
            status: status.to_string(),
            message: message.to_string(),
        };
    if let Ok(json) = serde_json::to_string(&payload) { app.trigger_global(EVT_DONE, Some(json)); }
    let _ = app.emit_all(EVT_DONE, payload);
}

fn run_layout_export(app: tauri::AppHandle, req: LayoutRequest) {
    let run_id = req.run_id;
    match assemble(&app, &req) {
        Ok(()) => emit_done(
            &app,
            run_id,
            "completed",
            &format!("Layout-preserving PDF saved to {}", req.output_path),
        ),
        Err(e) => {
            if CANCEL_FLAG.load(Ordering::SeqCst) || ACTIVE_RUN.load(Ordering::SeqCst) != run_id {
                emit_done(&app, run_id, "cancelled", "Export cancelled.");
            } else {
                warn!("Layout PDF export failed: {e}");
                emit_done(&app, run_id, "error", &e);
            }
        }
    }
}

fn assemble(app: &tauri::AppHandle, req: &LayoutRequest) -> Result<(), String> {
    if let (Ok(source), Ok(output)) = (
        std::fs::canonicalize(&req.source_pdf_path),
        std::fs::canonicalize(&req.output_path),
    ) {
        if source == output {
            return Err("Choose a different output path to preserve the original PDF".into());
        }
    }
    let run_id = req.run_id;
    let has_patterns = has_page_patterns(std::path::Path::new(&req.source_pdf_path))?;
    emit_progress(app, run_id, 2.0, "Preparing PDFium…");

    let lib_path = ensure_pdfium_library(app).map_err(|e| format!("PDFium library: {e}"))?;
    emit_progress(app, run_id, 4.0, "Binding PDFium…");
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| format!("PDFium bind failed: {e}"))?,
    );

    if has_patterns {
        return assemble_pattern_pdf(app, req, &pdfium);
    }

    emit_progress(app, run_id, 6.0, "Opening source PDF…");
    let private_source = if req.image_translation.is_some() || !req.image_replacements.is_empty() {
        emit_progress(app, run_id, 6.0, "Isolating shared image resources…");
        Some(super::image_resources::PrivateImageSource::create(
            std::path::Path::new(&req.source_pdf_path),
            run_id,
        )?)
    } else {
        None
    };
    let input = private_source
        .as_ref()
        .map(|source| source.path())
        .unwrap_or_else(|| std::path::Path::new(&req.source_pdf_path));
    let mut doc = pdfium
        .load_pdf_from_file(&input, None)
        .map_err(|e| format!("Failed to open PDF: {e}"))?;

    emit_progress(app, run_id, 8.0, "Loading fonts…");
    if !req.image_overlays.is_empty() {
        return Err("Whole-image OCR overlays cannot preserve backgrounds. Select Gemini Vision or the image translation service.".into());
    }
    let fonts = canonical::StyleFonts::load(&mut doc)?;
    let mut image_translator = req
        .image_translation
        .as_ref()
        .map(|options|super::image_translate::ImageTranslator::new(options).map(|translator|translator.for_run(run_id)))
        .transpose()?;

    let total = doc.pages().len() as usize;
    if req.pages.len() != total {
        return Err("PDF page count changed; re-extract this document".into());
    }
    let n_pages = total;
    let mut forms = Vec::new();

    for page_idx in 0..n_pages {
        if should_stop(run_id) {
            return Err("cancelled".to_string());
        }
        let page_spec = &req.pages[page_idx];
        let mut page = doc
            .pages()
            .get(page_idx as u16)
            .map_err(|e| e.to_string())?;

        let base_pct = 10.0 + (page_idx as f32 / (n_pages.max(1) as f32)) * 80.0;
        let report = |text: &str| {
            emit_progress(app, run_id, base_pct, text);
        };
        report(&format!(
            "Analyzing page {}/{} layout…",
            page_idx + 1,
            n_pages
        ));

        let layout = crate::document::pdf::canonical_page_layout(&page, page_idx)?;
        if page_spec.paragraphs.len() != layout.blocks.len() {
            return Err(format!(
                "Missing canonical blocks on page {}; re-extract this document",
                page_idx + 1
            ));
        }
        // Plan and validate all text before mutating the page. The only mapping
        // accepted is canonical block ID plus exact source text.
        let plan = canonical::plan_with_callouts(&page, &layout, page_spec, &fonts, &req.target_lang)?;
        forms.extend(canonical::form_removals(&plan));
        if let Some(translator) = image_translator.as_mut() {
            translator.translate_page(
                Some(app),
                &pdfium,
                &doc,
                &mut page,
                || should_stop(run_id),
                |message| report(message),
            )?;
        }

        // Service replacements update the existing XObject's bitmap, preserving
        // its complete transform, clipping and position in the content stream.
        let replacements: Vec<&ImageReplacement> = req
            .image_replacements
            .iter()
            .filter(|r| r.page_index == page_idx)
            .collect();
        if !replacements.is_empty() {
            report(&format!(
                "Placing {} translated image(s) on page {}/{}…",
                replacements.len(),
                page_idx + 1,
                n_pages
            ));
            apply_image_replacements(&mut page, &replacements)?;
        }

        report(&format!(
            "Applying translation on page {}/{}…",
            page_idx + 1,
            n_pages
        ));
        canonical::apply(&mut page, &plan, &fonts)?;

        let percent = 10.0 + (page_idx as f32 + 1.0) / (n_pages.max(1) as f32) * 80.0;
        emit_progress(
            app,
            run_id,
            percent,
            &format!("Finished page {}/{}", page_idx + 1, n_pages),
        );
    }

    if should_stop(run_id) {
        return Err("cancelled".to_string());
    }

    emit_progress(app, run_id, 95.0, "Writing output PDF...");
    let output = std::path::Path::new(&req.output_path);
    let name = output
        .file_name()
        .ok_or("Invalid output filename")?
        .to_string_lossy();
    let temporary = output.with_file_name(format!(".{name}.{}.tmp", req.run_id));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| format!("Create output file: {e}"))?;
    let saved = doc
        .save_to_writer(&mut file)
        .map_err(|e| e.to_string())
        .and_then(|_| file.sync_all().map_err(|e| e.to_string()));
    drop(file);
    if let Err(e) = saved {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Failed to save PDF: {e}"));
    }
    if let Err(e) = super::form_text::remove_translated_form_text(&temporary, &forms) {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    if should_stop(run_id) {
        let _ = std::fs::remove_file(&temporary);
        return Err("cancelled".into());
    }
    if let Err(e) = std::fs::rename(&temporary, output) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Finish output PDF: {e}"));
    }
    info!("Layout-preserving PDF written to {}", req.output_path);
    Ok(())
}

// PDFium page regeneration can replace pattern/shading graphics with solid
// fills. Do not publish a visibly damaged PDF while this format is unsupported.
fn has_page_patterns(path: &std::path::Path) -> Result<bool, String> {
    use lopdf::{content::Content, Document};
    let document = Document::load(path).map_err(|e| format!("Inspect source PDF: {e}"))?;
    for (page_number, page_id) in document.get_pages() {
        let bytes = document.get_page_content(page_id)
            .map_err(|e| format!("Inspect page {page_number} content: {e}"))?;
        let content = Content::decode(&bytes)
            .map_err(|e| format!("Inspect page {page_number} graphics: {e}"))?;
        if uses_pattern_or_shading(&content) {
            return Ok(true);
        }
    }
    for object in document.objects.values() {
        if let Ok(stream) = object.as_stream() {
            if stream.dict.get(b"Subtype").and_then(lopdf::Object::as_name).ok() != Some(b"Form") { continue; }
            let bytes = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
            let content = Content::decode(&bytes).map_err(|e| format!("Inspect vector Form graphics: {e}"))?;
            if uses_pattern_or_shading(&content) { return Ok(true); }
        }
    }
    Ok(false)
}

fn uses_pattern_or_shading(content: &lopdf::content::Content) -> bool {
    content.operations.iter().any(|op| {
        op.operator == "sh"
            || ((op.operator == "scn" || op.operator == "SCN")
                && op.operands.iter().any(|operand| matches!(operand, lopdf::Object::Name(_))))
    })
}

fn form_has_images(form: &PdfPageXObjectFormObject, depth: usize) -> Result<bool, String> {
    if depth > 16 { return Err("Vector Form nesting exceeds supported depth".into()); }
    for i in 0..form.len() {
        let object = form.get(i).map_err(|e| e.to_string())?;
        if object.as_image_object().is_some() { return Ok(true); }
        if let Some(child) = object.as_x_object_form_object() {
            if form_has_images(child, depth + 1)? { return Ok(true); }
        }
    }
    Ok(false)
}

fn build_pattern_pdf(pdfium: &Pdfium, req: &LayoutRequest, strict: bool) -> Result<Vec<u8>, String> {
    let source_pdf = pdfium.load_pdf_from_file(&req.source_pdf_path, None).map_err(|e| e.to_string())?;
    let source = lopdf::Document::load(&req.source_pdf_path).map_err(|e| e.to_string())?;
    let ids: Vec<_> = source.get_pages().values().copied().collect();
    if ids.len() != source_pdf.pages().len() as usize || (strict && req.pages.len() != ids.len()) {
        return Err("PDF page count changed; re-extract this document".into());
    }
    if !req.image_replacements.is_empty() || !req.image_overlays.is_empty() {
        return Err("Pattern-preserving export cannot yet replace image resources".into());
    }
    let mut overlay = pdfium.create_new_pdf().map_err(|e| e.to_string())?;
    let fonts = canonical::StyleFonts::load(&mut overlay)?;
    let mut contents = Vec::new();
    for (i, id) in ids.iter().copied().enumerate() {
        if strict && should_stop(req.run_id) { return Err("cancelled".into()); }
        let page = source_pdf.pages().get(i as u16).map_err(|e| e.to_string())?;
        let media = page.boundaries().media().map_err(|e| e.to_string())?.bounds;
        let cropped = page.boundaries().crop().ok().is_some_and(|c| c.bounds != media);
        if !matches!(page.rotation().map_err(|e| e.to_string())?, PdfPageRenderRotation::None)
            || media.left().value.abs() > 0.01 || media.bottom().value.abs() > 0.01 || cropped {
            return Err(format!("Page {} uses a rotated or offset page box unsupported by pattern-preserving export", i + 1));
        }
        if req.image_translation.is_some() {
            for object in page.objects().iter() {
                if object.as_image_object().is_some() || match object.as_x_object_form_object() {
                    Some(form) => form_has_images(form, 0)?, None => false,
                } {
                    return Err(format!("Page {} has images: pattern-preserving export cannot yet translate images safely", i + 1));
                }
            }
        }
        let layout = crate::document::pdf::canonical_page_layout(&page, i)?;
        let empty = LayoutPage { paragraphs: Vec::new(), block_ids: Vec::new() };
        let spec = req.pages.get(i).unwrap_or(&empty);
        if strict && spec.paragraphs.len() != layout.blocks.len() {
            return Err(format!("Missing canonical blocks on page {}; re-extract this document", i + 1));
        }
        let plan = canonical::plan_with_callouts(&page, &layout, spec, &fonts, &req.target_lang)?;
        let indices = canonical::text_object_indices(&plan)?;
        contents.push(super::lossless_pdf::page_content(&source, id, &page, &indices)?);
        let mut target = overlay.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(page.width(), page.height())).map_err(|e| e.to_string())?;
        target.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        canonical::draw_edits(&mut target, &plan, &fonts)?;
        target.regenerate_content().map_err(|e| e.to_string())?;
    }
    let overlay_bytes = overlay.save_to_bytes().map_err(|e| e.to_string())?;
    super::lossless_pdf::combine(source, &overlay_bytes, contents)
}

fn assemble_pattern_pdf(app: &tauri::AppHandle, req: &LayoutRequest, pdfium: &Pdfium) -> Result<(), String> {
    emit_progress(app, req.run_id, 10.0, "Preserving original vector graphics…");
    let bytes = build_pattern_pdf(pdfium, req, true)?;
    if should_stop(req.run_id) { return Err("cancelled".into()); }
    let output = std::path::Path::new(&req.output_path);
    let name = output.file_name().ok_or("Invalid output filename")?.to_string_lossy();
    let temporary = output.with_file_name(format!(".{name}.{}.tmp", req.run_id));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| e.to_string())?;
    use std::io::Write;
    let result = file.write_all(&bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = result { let _ = std::fs::remove_file(&temporary); return Err(e.to_string()); }
    if should_stop(req.run_id) { let _ = std::fs::remove_file(&temporary); return Err("cancelled".into()); }
    if let Err(e) = std::fs::rename(&temporary, output) { let _ = std::fs::remove_file(&temporary); return Err(format!("Finish output PDF: {e}")); }
    Ok(())
}

#[cfg(test)]
mod pattern_preflight_tests {
    use super::*;
    #[test]
    fn distinguishes_patterns_and_shadings_from_ordinary_colors() {
        use lopdf::content::{Content, Operation};
        let content = |operation| Content { operations: vec![operation] };
        assert!(uses_pattern_or_shading(&content(Operation::new("scn", vec![lopdf::Object::Name(b"P1".to_vec())]))));
        assert!(uses_pattern_or_shading(&content(Operation::new("sh", vec![lopdf::Object::Name(b"S1".to_vec())]))));
        assert!(!uses_pattern_or_shading(&content(Operation::new("scn", vec![lopdf::Object::Real(0.5)]))));
    }
    #[test]
    #[ignore = "Requires CTRANSLATE_TEST_PDF pointing to the pattern-graphics fixture"]
    fn pattern_fixture_is_detected_before_export() {
        let path = std::env::var("CTRANSLATE_TEST_PDF").unwrap();
        assert!(has_page_patterns(std::path::Path::new(&path)).unwrap());
    }
    #[test]
    #[ignore = "Requires CTRANSLATE_LAYOUT_REQUEST and CTRANSLATE_PDFIUM"]
    fn render_pattern_fixture() {
        let path = std::env::var("CTRANSLATE_LAYOUT_REQUEST").unwrap();
        let request: LayoutRequest = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let pdfium = Pdfium::new(Pdfium::bind_to_library(std::env::var("CTRANSLATE_PDFIUM").unwrap()).unwrap());
        let bytes = build_pattern_pdf(&pdfium, &request, false).unwrap();
        std::fs::write(&request.output_path, bytes).unwrap();
    }
}

struct FontMetrics<'a> {
    face: ttf_parser::Face<'a>,
    upem: f32,
    fallback: Option<ttf_parser::Face<'a>>,
}

impl<'a> FontMetrics<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        let face =
            ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("Font parse error: {e:?}"))?;
        let upem = face.units_per_em() as f32;
        Ok(Self {
            face,
            upem,
            fallback: None,
        })
    }

    fn char_w(&self, c: char, size: f32) -> f32 {
        if self.face.glyph_index(c).is_none() {
            if let Some(face) = &self.fallback {
                if let Some(advance) = face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)) {
                    return advance as f32 / face.units_per_em() as f32 * size;
                }
            }
        }
        let adv = self
            .face
            .glyph_index(c)
            .and_then(|g| self.face.glyph_hor_advance(g))
            .unwrap_or((self.upem * 0.5) as u16);
        adv as f32 / self.upem * size
    }

    fn has_glyph(&self, c: char) -> bool {
        self.face.glyph_index(c).is_some()
            || self
                .fallback
                .as_ref()
                .is_some_and(|face| face.glyph_index(c).is_some())
    }

    fn text_w(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.char_w(c, size)).sum()
    }
}

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

/// Split on whitespace, but also allow a break after `/`, `-` and similar
/// connectors. A translated cell like "220Vac,20A/tek-fazlı" has almost no
/// spaces to break at, so without these it hard-breaks mid-token or forces a
/// needless shrink.
///
/// Each piece carries whether a space preceded it in the source text, so
/// rejoining pieces on one line cannot invent a space inside "220Vac/20A".
fn break_opportunities(text: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for word in text.split_whitespace() {
        let mut piece = String::new();
        let mut first = true;
        for c in word.chars() {
            piece.push(c);
            if matches!(c, '/' | '-' | '，' | '、' | '·' | '\\' | '|') {
                out.push((std::mem::take(&mut piece), first));
                first = false;
            }
        }
        if !piece.is_empty() {
            out.push((piece, first));
        }
    }
    out
}

fn wrap_paragraph(m: &FontMetrics, text: &str, max_w: f32, size: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_w = 0.0f32;
    let space_w = m.char_w(' ', size);

    for (piece, space_before) in break_opportunities(text) {
        let word = piece.as_str();
        let word_w = m.text_w(word, size);
        // A piece produced by splitting inside a token ("220Vac/" + "20A") must
        // rejoin without a space, or the wrap would alter the text itself.
        let lead_w = if space_before { space_w } else { 0.0 };
        if line.is_empty() {
            if word_w <= max_w {
                line.push_str(word);
                line_w = word_w;
            } else {
                hard_break_word(m, word, max_w, size, &mut lines, &mut line, &mut line_w);
            }
        } else if line_w + lead_w + word_w <= max_w {
            if space_before {
                line.push(' ');
            }
            line.push_str(word);
            line_w += lead_w + word_w;
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

// Retain the pre-canonical planner's regression suite without shipping a
// second grouping/redaction implementation in the production export path.
#[cfg(test)]
#[allow(dead_code)]
#[path = "legacy_pdf_layout.rs"]
mod legacy;

fn apply_image_replacements(
    page: &mut PdfPage,
    replacements: &[&ImageReplacement],
) -> Result<(), String> {
    for replacement in replacements {
        let image = image::open(&replacement.image_path)
            .map_err(|e| format!("Read translated image: {e}"))?;
        let indices: Vec<_> = page
            .objects()
            .iter()
            .enumerate()
            .filter_map(|(i, object)| {
                if object.as_image_object().is_none() {
                    return None;
                }
                let b = object.bounds().ok()?;
                ((b.left().value - replacement.left).abs() < 0.2
                    && (b.right().value - replacement.right).abs() < 0.2
                    && (b.bottom().value - replacement.bottom).abs() < 0.2
                    && (b.top().value - replacement.top).abs() < 0.2)
                    .then_some(i)
            })
            .collect();
        if indices.len() != 1 {
            return Err("Translated image does not uniquely match a source image".into());
        }
        let mut object = page.objects().get(indices[0]).map_err(|e| e.to_string())?;
        let matrix = object.matrix().map_err(|e| e.to_string())?;
        object
            .as_image_object_mut()
            .ok_or("Source image disappeared")?
            .set_image(&image)
            .map_err(|e| e.to_string())?;
        object.reset_matrix(matrix).map_err(|e| e.to_string())?;
    }
    Ok(())
}
