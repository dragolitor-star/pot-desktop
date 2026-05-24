use log::info;
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};

/// Ensure that the PDFium dynamic library is present in the App Data folder.
/// If not, downloads it based on OS and architecture from bblanchon/pdfium-binaries
/// and extracts it.
pub fn ensure_pdfium_library(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let config_dir = dirs::config_dir().ok_or("Could not find user config directory")?;
    let app_dir = config_dir.join(&app_handle.config().tauri.bundle.identifier);
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;

    let lib_name = if cfg!(target_os = "windows") {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };

    let lib_path = app_dir.join(lib_name);
    if lib_path.exists() {
        return Ok(lib_path);
    }

    info!("PDFium dynamic library ({}) not found. Fetching from bblanchon/pdfium-binaries...", lib_name);
    download_pdfium(&app_dir, lib_name, &lib_path)?;

    Ok(lib_path)
}

fn download_pdfium(_app_dir: &Path, lib_name: &str, lib_path: &Path) -> Result<(), String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let asset_name = match (os, arch) {
        ("windows", "x86") => "pdfium-win-x86.zip",
        ("windows", "x86_64") => "pdfium-win-x64.zip",
        ("windows", "aarch64") => "pdfium-win-arm64.zip",
        ("macos", "x86_64") => "pdfium-mac-x64.tgz",
        ("macos", "aarch64") => "pdfium-mac-arm64.tgz",
        ("linux", "x86") => "pdfium-linux-x86.tgz",
        ("linux", "x86_64") => "pdfium-linux-x64.tgz",
        ("linux", "aarch64") => "pdfium-linux-arm64.tgz",
        ("linux", "arm") => "pdfium-linux-arm.tgz",
        _ => return Err(format!("Unsupported platform/architecture: {}-{}", os, arch)),
    };

    // Pinned stable release version (chromium/7843)
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7843/{}",
        asset_name
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    info!("Downloading PDFium from {}...", url);
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Failed to download PDFium: HTTP {}", response.status()));
    }

    let bytes = response.bytes().map_err(|e| e.to_string())?.to_vec();
    info!("Download complete. Size: {} bytes. Extracting dynamic library...", bytes.len());

    if asset_name.ends_with(".zip") {
        let reader = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;
        let mut found = false;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.name().ends_with(lib_name) {
                let mut out_file = std::fs::File::create(lib_path).map_err(|e| e.to_string())?;
                std::io::copy(&mut file, &mut out_file).map_err(|e| e.to_string())?;
                found = true;
                break;
            }
        }

        if !found {
            return Err(format!("Could not find {} inside downloaded zip archive", lib_name));
        }
    } else {
        // Extract tgz (tar + gzip)
        let tar_bytes = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(tar_bytes);
        let mut found = false;

        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?;
            if path.to_string_lossy().ends_with(lib_name) {
                // Ensure parent directory exists for unpack
                if let Some(parent) = lib_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                // Unpack expects a folder or takes the file path directly.
                // We'll write to a temp location or directly write the file bytes.
                let mut out_file = std::fs::File::create(lib_path).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
                found = true;
                break;
            }
        }

        if !found {
            return Err(format!("Could not find {} inside downloaded tgz archive", lib_name));
        }
    }

    info!("PDFium dynamic library extracted successfully to {:?}", lib_path);
    Ok(())
}

/// Extract all pages of text from the PDF file at path.
#[tauri::command]
pub fn extract_pdf_pages(app_handle: tauri::AppHandle, path: String) -> Result<Vec<String>, String> {
    let lib_path = ensure_pdfium_library(&app_handle)?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );

    let doc = pdfium.load_pdf_from_file(&path, None).map_err(|e| e.to_string())?;
    let mut pages_text = Vec::new();

    for page in doc.pages().iter() {
        let page_text = page.text().map_err(|e| e.to_string())?;
        pages_text.push(page_text.all());
    }

    Ok(pages_text)
}

/// Render a single PDF page to a cache PNG file for OCR processing, returning the path.
#[tauri::command]
pub fn render_pdf_page(
    app_handle: tauri::AppHandle,
    path: String,
    page_index: usize,
) -> Result<String, String> {
    let lib_path = ensure_pdfium_library(&app_handle)?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );

    let doc = pdfium.load_pdf_from_file(&path, None).map_err(|e| e.to_string())?;
    if page_index >= doc.pages().len() as usize {
        return Err(format!("Page index out of bounds: {} / {}", page_index, doc.pages().len()));
    }
    let page = doc.pages().get(page_index as u16).map_err(|e| e.to_string())?;

    let render_config = PdfRenderConfig::new().set_target_width(1200);
    let bitmap = page.render_with_config(&render_config).map_err(|e| e.to_string())?;
    let image = bitmap.as_image();

    let mut cache_dir_path = dirs::cache_dir().ok_or("Could not find cache directory")?;
    cache_dir_path.push(&app_handle.config().tauri.bundle.identifier);
    std::fs::create_dir_all(&cache_dir_path).map_err(|e| e.to_string())?;

    let cache_filename = format!("pdf_page_{}.png", page_index);
    let cache_file_path = cache_dir_path.join(cache_filename);

    image.save(&cache_file_path).map_err(|e| e.to_string())?;

    Ok(cache_file_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_platform_naming() {
        let os = std::env::consts::OS;
        let lib_name = if os == "windows" {
            "pdfium.dll"
        } else if os == "macos" {
            "libpdfium.dylib"
        } else {
            "libpdfium.so"
        };
        assert!(lib_name.contains("pdfium"));
    }
}
