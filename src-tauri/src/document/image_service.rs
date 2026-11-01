// External image-translation service integration.
//
// Bridges the layout-preserving PDF export to the standalone
// "layout-image-translator" FastAPI microservice (PaddleOCR-VL based). The
// service receives an embedded PDF image (PNG) via `POST /v1/translate` and
// returns the SAME image with its text translated in place (layout preserved,
// identical pixel dimensions) — so drawing the returned image back over the
// original image's bounding box keeps the page layout exactly.
//
// Strictly ADDITIVE and opt-in: nothing here runs unless the user picks the
// "Image translation service" method in the Document window. The service
// itself is NEVER modified or bundled — it is consumed over HTTP only
// (localhost by default, or a remote deployment; the URL is user-configurable).
// The existing OCR-overlay path (Task A2) is untouched and remains the default.

use log::info;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Per-image ceiling: PaddleOCR-VL inference + translation + inpainting can
/// take a while on CPU-only machines, so be generous before giving up.
const TRANSLATE_TIMEOUT: Duration = Duration::from_secs(300);
/// Health probe must be snappy — it only decides which error message to show.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(4);

/// A translated image to draw back onto the page at the ORIGINAL image's
/// bounding box (page-space PDF points, origin bottom-left — same coordinate
/// system as `image_text::PdfImageInfo`). Produced by the frontend after
/// calling `translate_image_via_service`; consumed by `pdf_layout::assemble`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageReplacement {
    pub page_index: usize,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
    #[serde(default)]
    pub image_path: String,
}

/// Quick reachability probe: true when the service answers its OpenAPI
/// endpoint. Never errors — "down" is an expected state the UI handles with
/// a friendly message (start command / URL hint), not an exception.
#[tauri::command]
pub fn check_image_service(base_url: String) -> bool {
    let url = openapi_endpoint(&base_url);
    let client = match reqwest::blocking::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get(&url).send() {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Send one extracted PDF image (the PNG at `cache_path`, produced by
/// `scan_pdf_images`) to the external service and store the translated result
/// under the app cache. Returns the absolute path of the translated PNG.
///
/// Languages are the service's own names ("auto", "English", "Turkish", …) —
/// the frontend maps Pot's language codes before invoking (see
/// `src/utils/imageService.js`). A failure translates ONE image only; the
/// frontend skips it and keeps the original, never aborting the export.
#[tauri::command]
pub fn translate_image_via_service(
    app_handle: tauri::AppHandle,
    base_url: String,
    cache_path: String,
    source_lang: String,
    target_lang: String,
) -> Result<String, String> {
    let bytes = std::fs::read(&cache_path)
        .map_err(|e| format!("Could not read extracted image {cache_path}: {e}"))?;
    let file_name = Path::new(&cache_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image.png".to_string());

    let out_dir = service_cache_dir(&app_handle)?;
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let out_path = out_dir.join(&file_name);

    let url = translate_endpoint(&base_url);
    let client = reqwest::blocking::Client::builder()
        .timeout(TRANSLATE_TIMEOUT)
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;

    let form = reqwest::blocking::multipart::Form::new()
        .part(
            "image",
            reqwest::blocking::multipart::Part::bytes(bytes)
                .file_name(file_name.clone())
                .mime_str("image/png")
                .map_err(|e| e.to_string())?,
        )
        .text("source_lang", source_lang)
        .text("target_lang", target_lang);

    let resp = client
        .post(&url)
        .multipart(form)
        .send()
        .map_err(|e| format!("Image service request failed ({url}): {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let snippet: String = body.chars().take(400).collect();
        return Err(format!("Image service returned HTTP {status}: {snippet}"));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let out_bytes = resp.bytes().map_err(|e| e.to_string())?;
    if !looks_like_image(&content_type, &out_bytes) {
        let snippet: String = String::from_utf8_lossy(&out_bytes).chars().take(400).collect();
        return Err(format!(
            "Image service returned a non-image response ({content_type}): {snippet}"
        ));
    }

    std::fs::write(&out_path, &out_bytes)
        .map_err(|e| format!("Could not write translated image: {e}"))?;
    info!(
        "Image service translated {file_name} ({} bytes) -> {}",
        out_bytes.len(),
        out_path.display()
    );
    Ok(out_path.to_string_lossy().into_owned())
}

fn service_cache_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let mut dir = dirs::cache_dir().ok_or("Could not find cache directory")?;
    dir.push(&app_handle.config().tauri.bundle.identifier);
    dir.push("pdf_image_service");
    Ok(dir)
}

// ── Pure helpers (unit-tested) ───────────────────────────────────────────────

/// Join the user-configured base URL with the service's single-call translate
/// endpoint, tolerating trailing slashes and a missing scheme.
pub(crate) fn translate_endpoint(base_url: &str) -> String {
    format!("{}/v1/translate", normalize_base(base_url))
}

pub(crate) fn openapi_endpoint(base_url: &str) -> String {
    format!("{}/openapi.json", normalize_base(base_url))
}

pub(crate) fn normalize_base(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return "http://127.0.0.1:8765".to_string();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

/// PNG/JPEG magic-byte + content-type sniff so a JSON error body saved as
/// ".png" can never silently end up drawn into the exported PDF.
pub(crate) fn looks_like_image(content_type: &str, bytes: &[u8]) -> bool {
    if content_type.starts_with("image/") {
        return true;
    }
    bytes.starts_with(&[0x89, b'P', b'N', b'G']) || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_tolerate_trailing_slash_and_missing_scheme() {
        assert_eq!(
            translate_endpoint("http://127.0.0.1:8765"),
            "http://127.0.0.1:8765/v1/translate"
        );
        assert_eq!(
            translate_endpoint("http://127.0.0.1:8765/"),
            "http://127.0.0.1:8765/v1/translate"
        );
        assert_eq!(
            translate_endpoint("localhost:8765"),
            "http://localhost:8765/v1/translate"
        );
        assert_eq!(
            openapi_endpoint("https://translator.example.com/"),
            "https://translator.example.com/openapi.json"
        );
    }

    #[test]
    fn empty_base_falls_back_to_default_localhost() {
        assert_eq!(normalize_base(""), "http://127.0.0.1:8765");
        assert_eq!(normalize_base("   "), "http://127.0.0.1:8765");
        assert_eq!(
            translate_endpoint("  "),
            "http://127.0.0.1:8765/v1/translate"
        );
    }

    #[test]
    fn image_sniff_accepts_real_images_rejects_json() {
        let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0xE0];
        assert!(looks_like_image("image/png", b"anything"));
        assert!(looks_like_image("application/octet-stream", &png));
        assert!(looks_like_image("", &jpeg));
        assert!(!looks_like_image("application/json", b"{\"detail\":\"err\"}"));
        assert!(!looks_like_image("text/plain", b"Internal Server Error"));
    }

    #[test]
    fn image_replacement_deserializes_from_camel_case() {
        let json = r#"{
            "pageIndex": 2,
            "left": 10.0,
            "bottom": 20.0,
            "right": 200.0,
            "top": 120.0,
            "imagePath": "C:/cache/p2_i0.png"
        }"#;
        let r: ImageReplacement = serde_json::from_str(json).unwrap();
        assert_eq!(r.page_index, 2);
        assert_eq!(r.image_path, "C:/cache/p2_i0.png");
        assert!((r.right - 200.0).abs() < f32::EPSILON);
    }
}
