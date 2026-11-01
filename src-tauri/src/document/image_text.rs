// Task A2 — In-image text translation (best-effort overlay).
//
// Strictly ADDITIVE and gated behind a UI toggle: when the "Translate text
// inside images" switch is OFF, NONE of this runs and the layout export stays
// byte-identical to A1. This module only EXTRACTS embedded raster images (so
// the frontend can OCR them with the EXISTING recognize stack and translate
// them with the EXISTING pipeline + glossary) and defines the `ImageOverlay`
// the layout export consumes to draw each translation back onto the page.
//
// Milestone 1 is a TEXT-OVERLAY: `pdf_layout::assemble` redacts each image's
// region with a white backing box and lays the translation into it
// (shrink-to-fit). True per-region inpainting (matching the original
// background and only covering the source glyphs) is intentionally deferred to
// a later commit — see the brief's "best-effort, leave full inpaint to a later
// commit". No new crate: reuses the already-present `pdfium-render` (with its
// default `image` feature) + `image` 0.25.

use log::{info, warn};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::document::pdf::ensure_pdfium_library;

const MIN_IMAGE_W_PT: f32 = 0.1;
const MIN_IMAGE_H_PT: f32 = 0.1;
const MAX_OCR_DIM_PX: u32 = 2000;
/// If Pdfium hangs on a bad raster, skip the page after this many seconds.
const PAGE_SCAN_TIMEOUT: Duration = Duration::from_secs(90);

const EVT_IMAGE_SCAN: &str = "image_scan://progress";

/// One embedded raster image on a page: its page-space bounding box (PDF
/// points, origin bottom-left) plus a cached PNG path for OCR (avoids shipping
/// huge base64 blobs over IPC).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfImageInfo {
    pub page_index: usize,
    pub index: usize,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
    pub cache_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageScanProgress {
    page_index: usize,
    page_count: usize,
    text: String,
}

/// A translated image-text block to draw back onto the page, produced by the
/// frontend after OCR + Chinese-detect + translate and consumed by
/// `pdf_layout::assemble`. Bounds are page-space PDF points (same coordinate
/// system as `PdfImageInfo`).
// Retained as a compatibility transport type. Production rejects whole-image
// overlays because they erase diagrams; new exports use image_translate.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOverlay {
    pub page_index: usize,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
    #[serde(default)]
    pub translated: String,
}

/// Scan every page once (single Pdfium load) and extract embedded images to
/// the app cache. Emits `image_scan://progress` while iterating so the UI does
/// not look frozen on large PDFs. The whole scan runs on a worker thread with
/// an overall timeout so a pathological raster cannot block export forever.
#[tauri::command]
pub fn scan_pdf_images(
    app_handle: tauri::AppHandle,
    path: String,
    page_count: usize,
) -> Result<Vec<PdfImageInfo>, String> {
    let app = app_handle.clone();
    let pdf_path = path.clone();
    let overall = Duration::from_secs(
        (page_count as u64)
            .saturating_mul(PAGE_SCAN_TIMEOUT.as_secs())
            .clamp(PAGE_SCAN_TIMEOUT.as_secs(), 1800),
    );
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = scan_pdf_images_inner(&app, &pdf_path, page_count);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(overall) {
        Ok(result) => result,
        Err(_) => Err(format!(
            "PDF image scan timed out after {}s",
            overall.as_secs()
        )),
    }
}

fn scan_pdf_images_inner(
    app_handle: &tauri::AppHandle,
    path: &str,
    page_count: usize,
) -> Result<Vec<PdfImageInfo>, String> {
    let lib_path = ensure_pdfium_library(app_handle)?;
    let cache_dir = image_scan_cache_dir(app_handle)?;
    if cache_dir.exists() {
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| e.to_string())?;

    let total = page_count.min(doc.pages().len() as usize);
    let mut all = Vec::new();

    for page_index in 0..total {
        emit_scan_progress(
            app_handle,
            page_index,
            total,
            &format!("Scanning images on page {}/{}…", page_index + 1, total),
        );
        let page = doc
            .pages()
            .get(page_index as u16)
            .map_err(|e| e.to_string())?;
        all.extend(collect_page_images(page_index, &page, &cache_dir));
    }

    info!(
        "PDF image scan complete: {} embedded images across {} pages",
        all.len(),
        total
    );
    Ok(all)
}

fn collect_page_images(page_index: usize, page: &PdfPage, cache_dir: &Path) -> Vec<PdfImageInfo> {
    let mut out = Vec::new();
    for (i, object) in page.objects().iter().enumerate() {
        let Some(image) = object.as_image_object() else {
            continue;
        };
        let bounds = match object.bounds() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let (left, bottom, right, top) = (
            bounds.left().value,
            bounds.bottom().value,
            bounds.right().value,
            bounds.top().value,
        );
        // Word numbering and warning badges are often only 10pt tall. A scan
        // for translation must include these; size is not evidence of no text.
        if !keep_image(right - left, top - bottom) {
            continue;
        }
        let dynimg = match image.get_raw_image() {
            Ok(im) => im,
            Err(e) => {
                warn!("Page {} image {i}: decode skipped ({e})", page_index);
                continue;
            }
        };
        let (w, h) = (dynimg.width(), dynimg.height());
        if w == 0 || h == 0 {
            continue;
        }
        let (nw, nh) = scaled_dims(w, h, MAX_OCR_DIM_PX);
        let dynimg = if (nw, nh) != (w, h) {
            dynimg.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
        } else {
            dynimg
        };

        let file_name = format!("p{page_index}_i{i}.png");
        let file_path = cache_dir.join(&file_name);
        if dynimg.save(&file_path).is_err() {
            continue;
        }
        out.push(PdfImageInfo {
            page_index,
            index: i,
            left,
            bottom,
            right,
            top,
            cache_path: file_path.to_string_lossy().into_owned(),
        });
    }
    out
}

/// Legacy single-page API — kept for callers/tests; prefer `scan_pdf_images`.
#[tauri::command]
pub fn extract_pdf_images(
    app_handle: tauri::AppHandle,
    path: String,
    page_index: usize,
) -> Result<Vec<PdfImageInfo>, String> {
    let lib_path = ensure_pdfium_library(&app_handle)?;
    let cache_dir = image_scan_cache_dir(&app_handle)?;
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    Ok(extract_page_images(
        &lib_path, &path, page_index, &cache_dir,
    )?)
}

pub(crate) fn extract_page_images(
    lib_path: &Path,
    pdf_path: &str,
    page_index: usize,
    cache_dir: &Path,
) -> Result<Vec<PdfImageInfo>, String> {
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );
    let doc = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| e.to_string())?;
    if page_index >= doc.pages().len() as usize {
        return Err(format!(
            "Page index out of bounds: {} / {}",
            page_index,
            doc.pages().len()
        ));
    }
    let page = doc
        .pages()
        .get(page_index as u16)
        .map_err(|e| e.to_string())?;

    Ok(collect_page_images(page_index, &page, cache_dir))
}

fn image_scan_cache_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let mut dir = dirs::cache_dir().ok_or("Could not find cache directory")?;
    dir.push(&app_handle.config().tauri.bundle.identifier);
    dir.push("pdf_image_scan");
    Ok(dir)
}

fn emit_scan_progress(app: &tauri::AppHandle, page_index: usize, page_count: usize, text: &str) {
    use tauri::Manager;
    let _ = app.emit_all(
        EVT_IMAGE_SCAN,
        ImageScanProgress {
            page_index,
            page_count,
            text: text.to_string(),
        },
    );
}

// ── Pure helpers (unit-tested) ───────────────────────────────────────────────

pub(crate) fn keep_image(width_pt: f32, height_pt: f32) -> bool {
    width_pt.is_finite()
        && height_pt.is_finite()
        && width_pt >= MIN_IMAGE_W_PT
        && height_pt >= MIN_IMAGE_H_PT
}

pub(crate) fn scaled_dims(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    if max_dim == 0 || w == 0 || h == 0 || (w <= max_dim && h <= max_dim) {
        return (w.max(1), h.max(1));
    }
    let longest = w.max(h) as f32;
    let scale = max_dim as f32 / longest;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    (nw, nh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_image_filters_small_decorations() {
        assert!(keep_image(120.0, 80.0));
        assert!(keep_image(MIN_IMAGE_W_PT, MIN_IMAGE_H_PT));
        assert!(keep_image(11.0, 11.0)); // Word chapter/step badge
        assert!(keep_image(300.0, 4.0));
        assert!(keep_image(10.0, 300.0));
        assert!(!keep_image(0.0, 10.0));
        assert!(!keep_image(f32::NAN, 10.0));
    }

    #[test]
    fn scaled_dims_keeps_small_images_unchanged() {
        assert_eq!(scaled_dims(800, 600, 2000), (800, 600));
        assert_eq!(scaled_dims(2000, 1000, 2000), (2000, 1000));
    }

    #[test]
    fn scaled_dims_shrinks_oversized_preserving_aspect() {
        let (w, h) = scaled_dims(4000, 2000, 2000);
        assert_eq!(w, 2000);
        assert_eq!(h, 1000);
        let (w2, h2) = scaled_dims(1000, 5000, 2000);
        assert_eq!(h2, 2000);
        assert_eq!(w2, 400);
    }

    #[test]
    fn scaled_dims_handles_degenerate_input() {
        assert_eq!(scaled_dims(0, 100, 2000), (1, 100));
        assert_eq!(scaled_dims(100, 0, 2000), (100, 1));
        assert_eq!(scaled_dims(9000, 9000, 0), (9000, 9000));
    }
}
