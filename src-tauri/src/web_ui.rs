//! Loopback-only browser interface for the existing PDF translation pipeline.
//! The bearer token is generated for each launch and is sent only in the URL
//! fragment; API requests also require the literal loopback Host header.
use crate::document::{pdf::PageLayout, pdf_layout::{LayoutPage, LayoutParagraph, LayoutRequest}, translate::DocTranslateRequest};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, io::Read, path::PathBuf, sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}};
use tauri::Manager;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use sha2::{Digest, Sha256};

const MAX_PDF: u64 = 100 * 1024 * 1024;
const MAX_JSON: u64 = 128 * 1024;
static URL: once_cell::sync::OnceCell<String> = once_cell::sync::OnceCell::new();
static SESSION: once_cell::sync::OnceCell<Arc<Mutex<State>>> = once_cell::sync::OnceCell::new();
static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    output: Option<PathBuf>,
    pages: Vec<PageLayout>,
    chapters: Vec<Vec<String>>,
    format: String,
    translations: HashMap<String, Value>,
    phase: String,
    message: String,
    percent: f64,
    run_id: u64,
    target: String,
    expected: usize,
    cache_model: String,
    source: String,
    media_path: Option<PathBuf>,
    media_phase: String,
    media_message: String,
    media_percent: f64,
    media_run_id: u64,
    media_cues: Vec<Value>,
    media_translations: HashMap<String, Value>,
    media_expected: usize,
    live_run_id: u64,
    live_state: String,
    live_message: String,
    live_original: String,
    live_translated: String,
    live_last_seen: Option<std::time::Instant>,
    text_run_id: u64,
    text_phase: String,
    text_original: String,
    text_translation: String,
    text_message: String,
    pending_ocr: Option<Value>,
    image_ocr_enabled: bool,
}

pub fn open(app: &tauri::AppHandle) -> Result<(), String> {
    let url = URL.get().ok_or("Web interface is not running")?;
    tauri::api::shell::open(&app.shell_scope(), url, None).map_err(|e| e.to_string())
}

pub fn start(app: tauri::AppHandle) -> Result<(), String> {
    let server = Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
    let address = server.server_addr().to_string();
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let url = format!("http://{address}/#{}", token);
    // Explicit opt-in for local integration tests; never enabled in release.
    #[cfg(debug_assertions)]
    if let Ok(path) = std::env::var("TURKTRANSLATE_TEST_URL_FILE") {
        std::fs::write(path, &url).map_err(|e| e.to_string())?;
    }
    URL.set(url).map_err(|_| "Web interface already started")?;
    let state = Arc::new(Mutex::new(State::default()));
    SESSION.set(Arc::clone(&state)).map_err(|_| "Web session already started")?;
    for name in ["document://progress", "document://batch", "document://done", "layout_export://progress", "layout_export://done", "layout_export://image_ocr", "media://progress", "media://cues", "media://done", "live_caption", "live_caption://state"] {
        let shared = Arc::clone(&state);
        app.listen_global(name, move |event| {
            let Some(payload) = event.payload().and_then(|s| serde_json::from_str::<Value>(s).ok()) else { return; };
            let Ok(mut s) = shared.lock() else { return; };
            let event_run = payload["runId"].as_u64();
            if event_run == Some(s.text_run_id) && s.text_run_id != 0 && name.starts_with("document://") {
                match name {
                    "document://progress" => s.text_message = payload["text"].as_str().unwrap_or("").into(),
                    "document://batch" => if let Some(result) = payload["results"].as_array().and_then(|v| v.first()) { s.text_translation = result["translation"].as_str().unwrap_or("").into(); if let Some(error) = result["error"].as_str() { s.text_message = error.into(); } },
                    "document://done" => { let complete = payload["status"] == "completed" && !s.text_translation.is_empty(); s.text_phase = if complete { "translated" } else { "error" }.into(); if !complete && (payload["status"] != "completed" || s.text_message.is_empty() || s.text_message == "Çeviri yapılıyor…") { s.text_message = payload["message"].as_str().unwrap_or("Çeviri tamamlanamadı.").into(); } },
                    _ => {}
                }
                return;
            }
            if event_run == Some(s.live_run_id) && s.live_run_id != 0 && (name == "live_caption" || name == "live_caption://state") {
                if name == "live_caption://state" { s.live_state = payload["state"].as_str().unwrap_or("error").into(); s.live_message = payload["message"].as_str().unwrap_or("").into(); }
                else { let partial = payload["partial"].as_str().unwrap_or(""); let final_text = payload["final"].as_str().unwrap_or(""); if !partial.is_empty() { s.live_original = partial.into(); s.live_translated.clear(); } if !final_text.is_empty() { s.live_original = final_text.into(); s.live_translated = payload["translated"].as_str().unwrap_or("").into(); } }
                return;
            }
            if event_run == Some(s.media_run_id) && s.media_run_id != 0 {
                match name {
                    "media://progress" => { s.media_percent = payload["percent"].as_f64().unwrap_or(0.0); s.media_message = payload["text"].as_str().unwrap_or("").into(); },
                    "media://cues" => { s.media_cues = payload["cues"].as_array().cloned().unwrap_or_default(); },
                    "media://done" => { s.media_phase = if payload["status"] == "completed" { "transcribed" } else { "error" }.into(); s.media_message = payload["message"].as_str().unwrap_or("").into(); s.media_percent = 100.0; },
                    "document://progress" if s.media_phase == "translating" => { let total = payload["totalUnique"].as_f64().unwrap_or(0.0); s.media_percent = if total > 0.0 { 100.0 * payload["doneUnique"].as_f64().unwrap_or(0.0) / total } else { 0.0 }; s.media_message = payload["text"].as_str().unwrap_or("").into(); },
                    "document://batch" if s.media_phase == "translating" => if let Some(results) = payload["results"].as_array() { for result in results { if let Some(text) = result["source"].as_str() { s.media_translations.insert(text.into(), result.clone()); } } },
                    "document://done" if s.media_phase == "translating" => { let complete = payload["status"] == "completed" && s.media_translations.len() == s.media_expected && s.media_translations.values().all(|v| v["translation"].as_str().is_some_and(|t| !t.is_empty())); s.media_phase = if complete { "translated" } else { "error" }.into(); s.media_message = if complete { "Altyazı çevirisi tamamlandı.".into() } else { payload["message"].as_str().unwrap_or("Altyazı çevirisi tamamlanamadı.").into() }; s.media_percent = 100.0; },
                    _ => {}
                }
                return;
            }
            if event_run != Some(s.run_id) { return; }
            match name {
                "layout_export://image_ocr" => { s.pending_ocr = Some(payload); }
                "document://progress" => {
                    s.message = payload["text"].as_str().unwrap_or("").to_string();
                    let total = payload["totalUnique"].as_f64().unwrap_or(0.0);
                    s.percent = if total > 0.0 { 100.0 * payload["doneUnique"].as_f64().unwrap_or(0.0) / total } else { 0.0 };
                }
                "document://batch" => if let Some(results) = payload["results"].as_array() {
                    for result in results { if let Some(source) = result["source"].as_str() {
                        if let Some(translated) = result["translation"].as_str().filter(|t| !t.is_empty()) {
                            let hash = cache_hash(&s.cache_model, &s.source, &s.target, source);
                            if let Err(e) = crate::cache::cache_set_translation(hash, s.cache_model.clone(), s.source.clone(), s.target.clone(), source.into(), translated.into()) { log::warn!("Web translation cache write failed: {e}"); }
                        }
                        s.translations.insert(source.to_string(), result.clone());
                    } }
                },
                "document://done" => {
                    let status = payload["status"].as_str().unwrap_or("error");
                    let complete = status == "completed" && s.translations.len() == s.expected && s.translations.values().all(|v| v["translation"].as_str().is_some_and(|t| !t.is_empty()));
                    s.phase = if complete { "translated" } else { "error" }.into();
                    s.message = if status == "completed" && !complete { "Bazı metinlerin çevirisi tamamlanamadı. Yeniden deneyin.".into() } else { payload["message"].as_str().unwrap_or("").into() };
                    s.percent = 100.0;
                }
                "layout_export://progress" => { s.percent = payload["percent"].as_f64().unwrap_or(0.0); s.message = payload["text"].as_str().unwrap_or("").into(); }
                "layout_export://done" => {
                    s.pending_ocr = None;
                    s.phase = if payload["status"] == "completed" { "ready" } else { "error" }.into();
                    s.message = payload["message"].as_str().unwrap_or("").into();
                    s.percent = 100.0;
                }
                _ => {}
            }
        });
    }
    let worker_app = app.clone();
    let watchdog = Arc::clone(&state);
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if let Ok(mut s) = watchdog.lock() {
            if matches!(s.live_state.as_str(), "starting" | "listening" | "transcribing") && s.live_last_seen.is_some_and(|last| last.elapsed() > std::time::Duration::from_secs(30)) {
                crate::media::live_caption::stop_live_caption(); s.live_state = "stopped".into(); s.live_message = "Tarayıcı bağlantısı kesildiği için canlı altyazı durduruldu.".into();
            }
        }
    });
    std::thread::spawn(move || {
        // Several bounded workers keep progress polling responsive during uploads.
        let server = Arc::new(server);
        for _ in 0..8 {
            let server = Arc::clone(&server);
            let address = address.clone();
            let token = token.clone();
            let state = Arc::clone(&state);
            let worker_app = worker_app.clone();
            std::thread::spawn(move || {
        for request in server.incoming_requests() {
            handle(request, &address, &token, &state, &worker_app);
        }
            });
        }
    });
    #[cfg(debug_assertions)]
    if std::env::var("TURKTRANSLATE_TEST_NO_OPEN").as_deref() == Ok("1") { return Ok(()); }
    open(&app)
}

pub fn cleanup() {
    crate::document::translate::cancel_document_translation();
    crate::document::pdf_layout::cancel_layout_export();
    crate::media::cancel_transcription();
    crate::media::live_caption::stop_live_caption();
    if let Some(state) = SESSION.get() {
        if let Ok(s) = state.lock() {
            if let Some(path) = &s.path { let _ = std::fs::remove_file(path); }
            if let Some(path) = &s.output { let _ = std::fs::remove_file(path); }
            if let Some(path) = &s.media_path { let _ = std::fs::remove_file(path); }
        }
    }
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn send(request: Request, status: u16, body: Vec<u8>, kind: &str) {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    for (name, value) in [("Content-Type", kind), ("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff"), ("Referrer-Policy", "no-referrer"), ("Content-Security-Policy", "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; worker-src 'self' blob:; img-src 'self' data: blob:; media-src 'self' blob: data:; font-src 'self' data:; connect-src 'self' https://pub-f6afb74f13c64cd89561b4714dca1c27.r2.dev; base-uri 'none'; frame-ancestors 'none'; form-action 'self'")] {
        if let Ok(h) = Header::from_bytes(name, value) { response.add_header(h); }
    }
    let _ = request.respond(response);
}
fn reply(request: Request, status: u16, value: Value) { send(request, status, value.to_string().into_bytes(), "application/json; charset=utf-8"); }
fn read_limited(request: &mut Request, max: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    request.as_reader().take(max + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max { return Err("Dosya boyutu sınırı aşıldı.".into()); }
    Ok(bytes)
}
fn language(code: &str) -> Option<&'static str> {
    match code { "auto" => Some("Auto"), "zh_cn" => Some("Simplified Chinese"), "en" => Some("English"), "tr" => Some("Turkish"), "de" => Some("German"), "fr" => Some("French"), "es" => Some("Spanish"), "ja" => Some("Japanese"), "ko" => Some("Korean"), _ => None }
}
fn is_busy(phase: &str) -> bool { matches!(phase, "translating" | "exporting" | "transcribing") }
fn session_busy(state: &State) -> bool { is_busy(&state.phase) || is_busy(&state.media_phase) || is_busy(&state.text_phase) }
fn cache_hash(model: &str, source: &str, target: &str, text: &str) -> String {
    format!("{:x}", Sha256::digest(format!("{model}\n{source}\n{target}\n{text}").as_bytes()))
}
fn authorized(host: Option<&str>, auth: Option<&str>, origin: Option<&str>, address: &str, token: &str) -> bool {
    host == Some(address) && auth == Some(format!("Bearer {token}").as_str())
        && origin.map_or(true, |value| value == format!("http://{address}"))
}

fn handle(mut request: Request, address: &str, token: &str, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) {
    if header(&request, "Host") != Some(address) { reply(request, 403, json!({"error":"Geçersiz adres"})); return; }
    let path = request.url().split('?').next().unwrap_or("").to_string();
    if request.method() == &Method::Get && path == "/api/asset" {
        let cookie_ok = header(&request, "Cookie").is_some_and(|cookies| cookies.split(';').any(|cookie| cookie.trim() == format!("tt_session={token}")));
        if !cookie_ok || header(&request, "Sec-Fetch-Site").is_some_and(|site| site != "same-origin") {
            reply(request, 401, json!({"error":"Oturum doğrulanamadı"})); return;
        }
        let url = tauri::Url::parse(&format!("http://{address}{}", request.url()));
        let file = url.ok().and_then(|url| url.query_pairs().find(|(key, _)| key == "path").map(|(_, value)| PathBuf::from(value.as_ref())));
        if let Some(file) = file.and_then(|file| file.canonicalize().ok()) {
            let permitted = [app.path_resolver().app_config_dir(), app.path_resolver().app_cache_dir()].into_iter().flatten()
                .filter_map(|dir| dir.canonicalize().ok()).any(|dir| file.starts_with(dir));
            let mime = match file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
                "png" => Some("image/png"), "jpg" | "jpeg" => Some("image/jpeg"), "webp" => Some("image/webp"),
                "gif" => Some("image/gif"), "svg" => Some("image/svg+xml"), _ => None,
            };
            if permitted {
                if let Some(mime) = mime {
                    if file.metadata().is_ok_and(|m| m.len() <= 32 * 1024 * 1024) {
                        if let Ok(bytes) = std::fs::read(file) { send(request, 200, bytes, mime); return; }
                    }
                }
            }
        }
        reply(request, 403, json!({"error":"Görsele erişim yok"})); return;
    }
    // Only packaged frontend assets are public. Reject traversal before the dev
    // asset resolver, which reads dist from disk, sees the path.
    if request.method() == &Method::Get && !path.starts_with("/api/") && !["/quick", "/app.css", "/app.js", "/subtitle.js", "/tesseract.js"].contains(&path.as_str()) &&
        !path.contains("..") && !path.contains('%') && !path.contains('\\') {
        let asset_path = if path == "/" { "/workspace.html" } else { &path };
        if let Some(asset) = app.asset_resolver().get(asset_path.into()) {
            send(request, 200, asset.bytes, &asset.mime_type); return;
        }
    }
    match (request.method(), path.as_str()) {
        (&Method::Get, "/quick") => { send(request, 200, include_bytes!("../../web-ui/index.html").to_vec(), "text/html; charset=utf-8"); return; }
        (&Method::Get, "/app.css") => { send(request, 200, include_bytes!("../../web-ui/app.css").to_vec(), "text/css; charset=utf-8"); return; }
        (&Method::Get, "/app.js") => { send(request, 200, include_bytes!("../../web-ui/app.js").to_vec(), "text/javascript; charset=utf-8"); return; }
        (&Method::Get, "/subtitle.js") => { send(request, 200, include_bytes!("../../src/utils/subtitle.js").to_vec(), "text/javascript; charset=utf-8"); return; }
        (&Method::Get, "/tesseract.js") => { send(request, 200, include_bytes!("../../web-ui/vendor/tesseract.esm.min.js").to_vec(), "text/javascript; charset=utf-8"); return; }
        (&Method::Get, "/worker.min.js") => { send(request, 200, include_bytes!("../../public/worker.min.js").to_vec(), "text/javascript; charset=utf-8"); return; }
        (&Method::Get, "/tesseract-core-simd-lstm.wasm.js") => { send(request, 200, include_bytes!("../../public/tesseract-core-simd-lstm.wasm.js").to_vec(), "text/javascript; charset=utf-8"); return; }
        _ => {}
    }
    if !authorized(header(&request, "Host"), header(&request, "Authorization"), header(&request, "Origin"), address, token) {
        reply(request, 401, json!({"error":"Oturum doğrulanamadı. Uygulamayı yeniden açın."})); return;
    }
    if request.method() == &Method::Post && path == "/api/session" {
        let response = Response::from_string("{}").with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
            .with_header(Header::from_bytes("Set-Cookie", format!("tt_session={token}; HttpOnly; SameSite=Strict; Path=/api/asset")).unwrap());
        let _ = request.respond(response); return;
    }
    let result = match (request.method().clone(), path.as_str()) {
        (Method::Post, "/api/upload") => upload(&mut request, state, app),
        (Method::Post, "/api/translate") => translate(&mut request, state, app),
        (Method::Post, "/api/export") => export(&mut request, state, app),
        (Method::Get, "/api/glossary") => glossary_list(),
        (Method::Post, "/api/glossary") => glossary_save(&mut request),
        (Method::Delete, "/api/glossary") => glossary_delete(&mut request),
        (Method::Post, "/api/media/upload") => media_upload(&mut request, state),
        (Method::Post, "/api/media/transcribe") => media_transcribe(&mut request, state, app),
        (Method::Post, "/api/media/translate") => media_translate(&mut request, state, app),
        (Method::Post, "/api/media/cancel") => media_cancel(state),
        (Method::Get, "/api/media/status") => media_status(state),
        (Method::Post, "/api/live/start") => live_start(&mut request, state, app),
        (Method::Post, "/api/live/stop") => live_stop(state, app),
        (Method::Post, "/api/live/passthrough") => live_passthrough(&mut request, app),
        (Method::Get, "/api/live/status") => live_status(state),
        (Method::Post, "/api/text/translate") => text_translate(&mut request, state, app),
        (Method::Post, "/api/text/cancel") => text_cancel(state),
        (Method::Get, "/api/text/status") => text_status(state),
        (Method::Post, "/api/bridge/connect") => crate::web_bridge::connect(),
        (Method::Post, "/api/bridge/upload") => browser_upload(&mut request, app),
        (Method::Post, "/api/bridge") => {
            let client = header(&request, "X-Bridge-Client").unwrap_or("").to_string();
            let body = read_limited(&mut request, 128 * 1024 * 1024).and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()));
            body.and_then(|value| crate::web_bridge::submit(app, &client, value))
        },
        (Method::Get, "/api/bridge/events") => {
            let client = header(&request, "X-Bridge-Client").unwrap_or("");
            let after = header(&request, "X-Bridge-Cursor").and_then(|s| s.parse().ok()).unwrap_or(0);
            crate::web_bridge::poll(client, after)
        },
        (Method::Get, "/api/ocr/job") => ocr_job(state),
        (Method::Post, "/api/ocr/submit") => ocr_submit(&mut request, state),
        (Method::Post, "/api/cancel") => cancel_document(state),
        (Method::Get, "/api/status") => status(state),
        (Method::Get, "/api/download") => {
            let output = state.lock().ok().and_then(|s| if s.phase == "ready" { s.output.clone().map(|p| (p, s.format.clone())) } else { None });
            if let Some((path, format)) = output { if let Ok(bytes) = std::fs::read(path) { send(request, 200, bytes, if format == "epub" { "application/epub+zip" } else { "application/pdf" }); return; } }
            Err("İndirilecek belge henüz hazır değil.".into())
        }
        _ => { reply(request, 404, json!({"error":"Sayfa bulunamadı"})); return; }
    };
    match result { Ok(value) => reply(request, 200, value), Err(error) => reply(request, 400, json!({"error":error})) }
}

fn detect_document(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.starts_with(b"%PDF-") { return Ok("pdf"); }
    if !bytes.starts_with(b"PK\x03\x04") { return Err("Geçerli bir PDF veya EPUB dosyası seçin.".into()); }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "Geçersiz EPUB arşivi")?;
    if zip.len() > 20000 { return Err("EPUB çok fazla dosya içeriyor.".into()); }
    let mut total = 0_u64;
    for i in 0..zip.len() {
        let item = zip.by_index(i).map_err(|e| format!("EPUB okunamadı: {e}"))?;
        total = total.checked_add(item.size()).ok_or("EPUB boyutu geçersiz")?;
        if total > 256 * 1024 * 1024 { return Err("EPUB açılmış boyutu 256 MB sınırını aşıyor.".into()); }
    }
    let mut mimetype = String::new();
    zip.by_name("mimetype").map_err(|_| "EPUB mimetype dosyası eksik")?.take(128).read_to_string(&mut mimetype).map_err(|e| e.to_string())?;
    if mimetype.trim() != "application/epub+zip" { return Err("Geçerli bir EPUB dosyası seçin.".into()); }
    Ok("epub")
}

fn epub_protected(text: &str) -> bool {
    let text = text.trim();
    if text.chars().count() < 3 || crate::document::pdf::contains_han(text) { return false; }
    if (text.starts_with("$$") && text.ends_with("$$")) || text.contains("\\frac{") || crate::document::pdf::is_code_line(text) || crate::document::pdf::is_formula(text) { return true; }
    let meaningful: Vec<_> = text.chars().filter(|c| !c.is_whitespace()).collect();
    !meaningful.is_empty() && meaningful.iter().filter(|c| c.is_alphabetic()).count() * 10 < meaningful.len() * 3
}

fn browser_upload(request: &mut Request, app: &tauri::AppHandle) -> Result<Value, String> {
    let client = header(request, "X-Bridge-Client").unwrap_or("");
    // Validate the session before creating any file. UUID parsing also excludes traversal.
    uuid::Uuid::parse_str(client).map_err(|_| "Invalid browser session")?;
    crate::web_bridge::poll(client, 0)?;
    let name = header(request, "X-File-Name").unwrap_or("document.bin");
    let name = tauri::Url::parse(&format!("http://localhost/?name={name}")).map_err(|e| e.to_string())?
        .query_pairs().find(|(k, _)| k == "name").map(|(_, v)| v.into_owned()).unwrap_or_default();
    let name = name.rsplit(['/', '\\']).next().unwrap_or("file.bin");
    let name: String = name.chars().map(|c| if c.is_alphanumeric() || ['.', '-', '_', ' '].contains(&c) { c } else { '_' }).take(160).collect();
    let dir = app.path_resolver().app_cache_dir().ok_or("Cache directory missing")?.join("browser-uploads").join(client);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}-{name}", uuid::Uuid::new_v4()));
    let limit = 2 * 1024 * 1024 * 1024u64;
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let copied = std::io::copy(&mut request.as_reader().take(limit + 1), &mut file).map_err(|e| e.to_string());
    drop(file);
    match copied {
        Ok(bytes) if bytes > 0 && bytes <= limit => Ok(json!({"path":path.to_string_lossy()})),
        result => { let _ = std::fs::remove_file(path); Err(result.err().unwrap_or("Dosya boş veya 2 GB sınırını aşıyor.".into())) }
    }
}

fn upload(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    { let s = state.lock().map_err(|e| e.to_string())?; if session_busy(&s) { return Err("İşlem sürerken başka dosya yüklenemez.".into()); } }
    let bytes = read_limited(request, MAX_PDF)?;
    let format = detect_document(&bytes)?;
    let dir = std::env::temp_dir().join("turktranslate-web");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.{format}", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let (pages, chapters) = if format == "pdf" {
        match crate::document::pdf::extract_pdf_layout(app.clone(), path.to_string_lossy().into_owned()) {
            Ok(pages) if !pages.is_empty() => (pages, Vec::new()),
            Ok(_) => { let _ = std::fs::remove_file(&path); return Err("PDF içinde sayfa bulunamadı.".into()); },
            Err(e) => { let _ = std::fs::remove_file(&path); return Err(e); }
        }
    } else {
        match crate::document::epub::extract_epub(path.to_string_lossy().into_owned()) {
            Ok(chapters) => (Vec::new(), chapters.into_iter().map(|chapter| chapter.split("\n\n").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()).collect()),
            Err(e) => { let _ = std::fs::remove_file(&path); return Err(e); }
        }
    };
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if let Some(old) = s.path.replace(path) { let _ = std::fs::remove_file(old); }
    if let Some(old) = s.output.take() { let _ = std::fs::remove_file(old); }
    s.pages = pages; s.chapters = chapters; s.format = format.into(); s.translations.clear(); s.phase = "loaded".into(); s.message.clear(); s.percent = 0.0; s.image_ocr_enabled = false;
    Ok(json!({"format":format,"pages":if format == "pdf" { s.pages.len() } else { s.chapters.len() }, "blocks":if format == "pdf" { s.pages.iter().map(|p| p.blocks.len()).sum::<usize>() } else { s.chapters.iter().map(Vec::len).sum() }}))
}

fn translate(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let key = body["apiKey"].as_str().unwrap_or("").trim();
    let model = body["model"].as_str().unwrap_or("").trim();
    let src = body["source"].as_str().unwrap_or("auto");
    let target = body["target"].as_str().unwrap_or("tr");
    if key.is_empty() || key.len() > 512 { return Err("Gemini API anahtarını girin.".into()); }
    if model.is_empty() || model.len() > 100 || !model.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.') { return Err("Geçersiz Gemini model adı.".into()); }
    let from_label = language(src).ok_or("Kaynak dil desteklenmiyor")?;
    let to_label = language(target).filter(|_| target != "auto").ok_or("Hedef dil desteklenmiyor")?;
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if (s.pages.is_empty() && s.chapters.is_empty()) || session_busy(&s) { return Err("Önce bir belge yükleyin ve devam eden işlemi bitirin.".into()); }
    let mut seen = HashSet::new();
    let units: Vec<String> = if s.format == "epub" { s.chapters.iter().flatten().filter(|t| !epub_protected(t)).cloned().collect() }
        else { s.pages.iter().flat_map(|p| &p.blocks).filter(|b| !b.protected).map(|b| b.translation_text.clone()).collect() };
    let texts: Vec<String> = units.into_iter().filter(|text| !text.trim().is_empty() && seen.insert(text.clone())).collect();
    if texts.is_empty() && !(s.format == "pdf" && body["translateImages"].as_bool().unwrap_or(false)) { return Err("Çevrilecek metin bulunamadı. Görsellerdeki yazıları çevirmeyi deneyin.".into()); }
    let glossary_source = if src == "auto" {
        if texts.iter().any(|text| crate::document::pdf::contains_han(text)) { "zh_cn" } else { "en" }
    } else { src };
    let entries = crate::glossary::get_active_glossary(glossary_source.into(), target.into(), Some("document".into()))
        .map_err(|e| format!("Terim sözlüğü okunamadı: {e}"))?;
    let signature = serde_json::to_vec(&entries.iter().map(|e| (&e.source_term, &e.target_term, e.case_sensitive)).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
    let model_key = format!("gemini:{model}:web-canonical-v1:{:x}", Sha256::digest(signature));
    let glossary = entries.into_iter().map(|entry| crate::document::translate::GlossaryPair { source_term: entry.source_term, target_term: entry.target_term }).collect();
    let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
    s.run_id = run_id; s.phase = "translating".into(); s.message = "Çeviri başlatılıyor…".into(); s.percent = 0.0; s.translations.clear(); s.target = target.into(); s.source = src.into(); s.cache_model = model_key.clone(); s.expected = texts.len();
    let mut uncached = Vec::new();
    for text in texts {
        let cached = match crate::cache::cache_get_translation(cache_hash(&model_key, src, target, &text)) {
            Ok(value) => value,
            Err(e) => { log::warn!("Web translation cache read failed: {e}"); None }
        };
        if let Some(value) = cached.filter(|v| !crate::document::translate::residual_cjk(v, to_label) && !crate::document::translate::protected_token_loss(&text, v)) {
            s.translations.insert(text.clone(), json!({"source":text,"translation":value,"error":null}));
        } else { uncached.push(text); }
    }
    if uncached.is_empty() { s.phase = "translated".into(); s.message = "Çeviriler önbellekten yüklendi.".into(); s.percent = 100.0; return Ok(json!({"ok":true})); }
    drop(s);
    crate::document::translate::translate_document(app.clone(), DocTranslateRequest {
        run_id, texts: uncached, api_key: key.into(), api_base: "https://generativelanguage.googleapis.com/v1beta/models/".into(), model: model.into(),
        from_label: from_label.into(), to_label: to_label.into(), chars_per_request: 6000,
        rpm_limit: body["rpm"].as_u64().unwrap_or(10).clamp(1, 600) as u32, glossary,
    });
    Ok(json!({"ok":true}))
}

fn export(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let body = read_limited(request, MAX_JSON)?;
    let options: Value = if body.is_empty() { json!({}) } else { serde_json::from_slice(&body).map_err(|e| e.to_string())? };
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if s.phase != "translated" { return Err("Önce çeviriyi tamamlayın.".into()); }
    if session_busy(&s) { return Err("Devam eden işlemi bitirin.".into()); }
    let source = s.path.clone().ok_or("PDF bulunamadı")?;
    let output = source.with_file_name(format!("{}-translated.{}", uuid::Uuid::new_v4(), s.format));
    if s.format == "epub" {
        let mut translations = HashMap::new();
        for text in s.chapters.iter().flatten().filter(|text| !epub_protected(text)) {
            let translated = s.translations.get(text).and_then(|v| v["translation"].as_str()).ok_or("EPUB çevirisi eksik")?;
            translations.insert(text.clone(), translated.to_string());
        }
        let bilingual = options["bilingual"].as_bool().unwrap_or(false);
        let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
        if let Some(old) = s.output.replace(output.clone()) { let _ = std::fs::remove_file(old); }
        s.run_id = run_id; s.phase = "exporting".into(); s.message = "EPUB hazırlanıyor…".into(); s.percent = 0.0;
        let shared = Arc::clone(state);
        drop(s);
        std::thread::spawn(move || {
            let result = crate::document::epub::assemble_epub(source.to_string_lossy().into_owned(), output.to_string_lossy().into_owned(), translations, bilingual);
            if let Ok(mut s) = shared.lock() { if s.run_id == run_id {
                match result { Ok(()) => { s.phase = "ready".into(); s.message = "EPUB hazır.".into(); }, Err(e) => { s.phase = "error".into(); s.message = e; } }
                s.percent = 100.0;
            } }
        });
        return Ok(json!({"ok":true}));
    }
    let mut pages = Vec::new();
    for page in &s.pages {
        let mut paragraphs = Vec::new();
        let mut block_ids = Vec::new();
        for block in &page.blocks {
            let translated = if block.protected || block.translation_text.trim().is_empty() { block.text.clone() } else {
                s.translations.get(&block.translation_text).and_then(|v| v["translation"].as_str()).ok_or_else(|| format!("Çeviri eksik: {}", block.block_id))?.to_string()
            };
            paragraphs.push(LayoutParagraph { original: block.text.clone(), translated });
            block_ids.push(block.block_id.clone());
        }
        pages.push(LayoutPage { paragraphs, block_ids });
    }
    let image_translation = if options["translateImages"].as_bool().unwrap_or(false) {
        let (key, model, source_lang, target_lang, rpm_limit) = media_config(&options)?;
        if source_lang != s.source || target_lang != s.target { return Err("Görsel çevirisinin dilleri belge çevirisiyle aynı olmalıdır.".into()); }
        let glossary_source = if source_lang == "auto" { "zh_cn" } else { &source_lang };
        let glossary = crate::glossary::get_active_glossary(glossary_source.into(), target_lang.clone(), Some("document".into()))?.into_iter().map(|entry| crate::document::translate::GlossaryPair { source_term: entry.source_term, target_term: entry.target_term }).collect();
        Some(crate::document::image_translate::ImageTranslateOptions { api_key: key, api_base: "https://generativelanguage.googleapis.com/v1beta/models/".into(), model, source_lang, target_lang, rpm_limit, glossary })
    } else { None };
    let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
    let target = s.target.clone();
    if let Some(old) = s.output.replace(output.clone()) { let _ = std::fs::remove_file(old); }
    s.run_id = run_id; s.phase = "exporting".into(); s.percent = 0.0; s.message = "PDF hazırlanıyor…".into(); s.pending_ocr = None; s.image_ocr_enabled = image_translation.is_some();
    drop(s);
    crate::document::pdf_layout::export_layout_pdf(app.clone(), LayoutRequest { run_id, source_pdf_path: source.to_string_lossy().into_owned(), output_path: output.to_string_lossy().into_owned(), pages, target_lang: target, image_translation, image_overlays: vec![], image_replacements: vec![] });
    Ok(json!({"ok":true}))
}

fn ocr_job(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    Ok(s.pending_ocr.take().unwrap_or(Value::Null))
}
fn ocr_submit(request: &mut Request, state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, 8 * 1024 * 1024)?).map_err(|e| e.to_string())?;
    let s = state.lock().map_err(|e| e.to_string())?;
    if s.phase != "exporting" || body["runId"].as_u64() != Some(s.run_id) { return Err("OCR işlemi artık etkin değil.".into()); }
    let job_id = body["jobId"].as_u64().ok_or("OCR görevi eksik")?;
    let document = if body["document"].is_null() { None } else { Some(serde_json::from_value(body["document"].clone()).map_err(|e| format!("OCR sonucu geçersiz: {e}"))?) };
    let error = body["error"].as_str().map(str::to_string);
    let accepted = crate::document::image_ocr::submit_layout_image_ocr(job_id, document, error)?;
    Ok(json!({"accepted":accepted}))
}

fn cancel_document(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    match s.phase.as_str() {
        "translating" => crate::document::translate::cancel_document_translation(),
        "exporting" if s.format == "pdf" => crate::document::pdf_layout::cancel_layout_export(),
        _ => return Err("Durdurulacak belge işlemi yok.".into()),
    }
    Ok(json!({"ok":true}))
}

fn status(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let pages: Vec<Value> = if s.format == "epub" { s.chapters.iter().enumerate().map(|(i, chapter)| json!({"index":i+1,"blocks":chapter.iter().map(|text| json!({"original":text,"protected":epub_protected(text),"translation":s.translations.get(text).and_then(|v| v["translation"].as_str()),"error":s.translations.get(text).and_then(|v| v["error"].as_str())})).collect::<Vec<_>>()})).collect() } else { s.pages.iter().map(|p| json!({"index":p.page_index + 1,"blocks":p.blocks.iter().map(|b| json!({"original":b.translation_text,"protected":b.protected,"translation":s.translations.get(&b.translation_text).and_then(|v| v["translation"].as_str()),"error":s.translations.get(&b.translation_text).and_then(|v| v["error"].as_str())})).collect::<Vec<_>>()})).collect() };
    Ok(json!({"phase":s.phase,"format":s.format,"message":s.message,"percent":s.percent,"pages":pages,"imageOcr":s.image_ocr_enabled}))
}

fn glossary_list() -> Result<Value, String> {
    let entries = crate::glossary::list_glossaries(None)?;
    serde_json::to_value(entries).map_err(|e| e.to_string())
}
fn glossary_save(request: &mut Request) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let entry = serde_json::from_value(body.get("entry").cloned().ok_or("Terim bilgisi eksik")?).map_err(|e| e.to_string())?;
    if let Some(id) = body.get("id").and_then(Value::as_i64) {
        crate::glossary::update_glossary_entry(id, entry)?;
        Ok(json!({"id":id}))
    } else {
        Ok(json!({"id":crate::glossary::add_glossary_entry(entry)?}))
    }
}
fn glossary_delete(request: &mut Request) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let id = body["id"].as_i64().filter(|id| *id > 0).ok_or("Geçersiz terim numarası")?;
    crate::glossary::delete_glossary_entry(id)?;
    Ok(json!({"ok":true}))
}

fn media_upload(request: &mut Request, state: &Arc<Mutex<State>>) -> Result<Value, String> {
    { let s = state.lock().map_err(|e| e.to_string())?; if session_busy(&s) { return Err("Devam eden işlemi bitirin.".into()); } }
    let extension = header(request, "X-File-Extension").unwrap_or("").to_ascii_lowercase();
    let allowed = ["mp3", "wav", "m4a", "aac", "ogg", "oga", "flac", "aiff", "aif", "opus", "mp4", "m4v", "mov", "mkv", "webm", "avi", "mpeg", "mpg", "wmv", "flv", "3gp"];
    if !allowed.contains(&extension.as_str()) { return Err("Desteklenen bir ses veya video dosyası seçin.".into()); }
    const LIMIT: u64 = 2 * 1024 * 1024 * 1024;
    if request.body_length().is_some_and(|length| length as u64 > LIMIT) { return Err("Medya dosyası 2 GB sınırını aşıyor.".into()); }
    let dir = std::env::temp_dir().join("turktranslate-web");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    let result = std::io::copy(&mut request.as_reader().take(LIMIT + 1), &mut file);
    drop(file);
    let size = match result { Ok(size) if size > 0 && size <= LIMIT => size, Ok(_) => { let _ = std::fs::remove_file(&path); return Err("Dosya boş veya 2 GB sınırından büyük.".into()); }, Err(e) => { let _ = std::fs::remove_file(&path); return Err(format!("Medya dosyası kaydedilemedi: {e}")); } };
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if let Some(old) = s.media_path.replace(path) { let _ = std::fs::remove_file(old); }
    s.media_cues.clear(); s.media_translations.clear(); s.media_phase = "loaded".into(); s.media_message.clear(); s.media_percent = 0.0; s.media_run_id = 0;
    Ok(json!({"size":size}))
}

fn media_config(body: &Value) -> Result<(String, String, String, String, u32), String> {
    let key = body["apiKey"].as_str().unwrap_or("").trim();
    let model = body["model"].as_str().unwrap_or("").trim();
    let source = body["source"].as_str().unwrap_or("auto");
    let target = body["target"].as_str().unwrap_or("tr");
    if key.is_empty() || key.len() > 512 { return Err("Gemini API anahtarını girin.".into()); }
    if model.is_empty() || model.len() > 100 || !model.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.') { return Err("Geçersiz Gemini model adı.".into()); }
    if language(source).is_none() || language(target).is_none() || target == "auto" { return Err("Desteklenmeyen dil seçimi.".into()); }
    Ok((key.into(), model.into(), source.into(), target.into(), body["rpm"].as_u64().unwrap_or(10).clamp(1, 600) as u32))
}

fn media_transcribe(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let (key, model, source, _, _) = media_config(&body)?;
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if session_busy(&s) { return Err("Devam eden işlemi bitirin.".into()); }
    let path = s.media_path.as_ref().ok_or("Önce bir medya dosyası yükleyin")?.to_string_lossy().into_owned();
    let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
    s.media_run_id = run_id; s.media_phase = "transcribing".into(); s.media_message = "Medya hazırlanıyor…".into(); s.media_percent = 0.0; s.media_cues.clear(); s.media_translations.clear();
    drop(s);
    crate::media::transcribe_media(app.clone(), crate::media::TranscribeRequest { run_id, path, api_key: key, api_base: "https://generativelanguage.googleapis.com/v1beta/models/".into(), model, language_hint: language(&source).unwrap_or("Auto").into() });
    Ok(json!({"ok":true}))
}

fn media_translate(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let (key, model, source, target, rpm) = media_config(&body)?;
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if session_busy(&s) { return Err("Devam eden işlemi bitirin.".into()); }
    let mut seen = HashSet::new();
    let texts: Vec<String> = s.media_cues.iter().filter_map(|cue| cue["text"].as_str()).filter(|t| !t.trim().is_empty() && seen.insert((*t).to_string())).map(str::to_string).collect();
    if texts.is_empty() { return Err("Önce konuşmayı metne dönüştürün.".into()); }
    let glossary_source = if source == "auto" { if texts.iter().any(|t| crate::document::pdf::contains_han(t)) { "zh_cn" } else { "en" } } else { &source };
    let glossary = crate::glossary::get_active_glossary(glossary_source.into(), target.clone(), Some("document".into()))?.into_iter().map(|e| crate::document::translate::GlossaryPair { source_term: e.source_term, target_term: e.target_term }).collect();
    let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
    s.media_run_id = run_id; s.media_phase = "translating".into(); s.media_message = "Altyazılar çevriliyor…".into(); s.media_percent = 0.0; s.media_expected = texts.len(); s.media_translations.clear();
    drop(s);
    crate::document::translate::translate_document(app.clone(), DocTranslateRequest { run_id, texts, api_key: key, api_base: "https://generativelanguage.googleapis.com/v1beta/models/".into(), model, from_label: language(&source).unwrap_or("Auto").into(), to_label: language(&target).unwrap_or("Turkish").into(), chars_per_request: 6000, rpm_limit: rpm, glossary });
    Ok(json!({"ok":true}))
}

fn media_cancel(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let phase = state.lock().map_err(|e| e.to_string())?.media_phase.clone();
    match phase.as_str() { "transcribing" => crate::media::cancel_transcription(), "translating" => crate::document::translate::cancel_document_translation(), _ => return Err("Durdurulacak medya işlemi yok.".into()) }
    Ok(json!({"ok":true}))
}

fn media_status(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let cues: Vec<_> = s.media_cues.iter().map(|cue| { let text = cue["text"].as_str().unwrap_or(""); json!({"start":cue["start"],"end":cue["end"],"original":text,"translated":s.media_translations.get(text).and_then(|v| v["translation"].as_str()),"error":s.media_translations.get(text).and_then(|v| v["error"].as_str())}) }).collect();
    Ok(json!({"phase":s.media_phase,"message":s.media_message,"percent":s.media_percent,"cues":cues}))
}

fn live_start(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let (key, model, source, target, _) = media_config(&body)?;
    let glossary_source = if source == "auto" { "en" } else { &source };
    let glossary = crate::glossary::get_active_glossary(glossary_source.into(), target.clone(), Some("document".into()))?.into_iter().map(|e| crate::media::live_caption::GlossaryPair { source_term: e.source_term, target_term: e.target_term }).collect();
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if matches!(s.live_state.as_str(), "starting" | "listening" | "transcribing") { return Err("Canlı altyazı zaten çalışıyor.".into()); }
    let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
    s.live_run_id = run_id; s.live_state = "starting".into(); s.live_message = "Sistem sesi hazırlanıyor…".into(); s.live_original.clear(); s.live_translated.clear(); s.live_last_seen = Some(std::time::Instant::now());
    drop(s);
    if body["overlay"].as_bool().unwrap_or(false) { crate::window::live_caption_window(); }
    crate::media::live_caption::start_live_caption(app.clone(), crate::media::live_caption::LiveCaptionRequest { run_id, api_key: key, api_base: "https://generativelanguage.googleapis.com/v1beta/models/".into(), model, from_label: language(&source).unwrap_or("Auto").into(), to_label: language(&target).unwrap_or("Turkish").into(), chunk_seconds: body["chunkSeconds"].as_f64().unwrap_or(5.0).clamp(2.0, 15.0), glossary });
    Ok(json!({"ok":true}))
}
fn live_stop(state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    crate::media::live_caption::stop_live_caption();
    s.live_state = "stopped".into(); s.live_message = "Canlı altyazı durduruldu.".into();
    drop(s);
    if let Some(window) = app.get_window("live_caption") { window.close().map_err(|e| e.to_string())?; }
    Ok(json!({"ok":true}))
}
fn live_passthrough(request: &mut Request, app: &tauri::AppHandle) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let enabled = body["enabled"].as_bool().ok_or("Geçersiz seçim")?;
    app.emit_all("live_caption://passthrough", json!({"enabled":enabled})).map_err(|e| e.to_string())?;
    Ok(json!({"ok":true}))
}
fn live_status(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.live_last_seen = Some(std::time::Instant::now());
    Ok(json!({"state":s.live_state,"message":s.live_message,"original":s.live_original,"translated":s.live_translated}))
}

fn text_translate(request: &mut Request, state: &Arc<Mutex<State>>, app: &tauri::AppHandle) -> Result<Value, String> {
    let body: Value = serde_json::from_slice(&read_limited(request, MAX_JSON)?).map_err(|e| e.to_string())?;
    let (key, model, source, target, rpm) = media_config(&body)?;
    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() || text.len() > 100_000 { return Err("Çevirmek için en fazla 100 KB metin girin.".into()); }
    let glossary_source = if source == "auto" { if crate::document::pdf::contains_han(text) { "zh_cn" } else { "en" } } else { &source };
    let glossary = crate::glossary::get_active_glossary(glossary_source.into(), target.clone(), None)?.into_iter().map(|e| crate::document::translate::GlossaryPair { source_term: e.source_term, target_term: e.target_term }).collect();
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if session_busy(&s) { return Err("Devam eden işlemi bitirin.".into()); }
    let run_id = NEXT_RUN.fetch_add(1, Ordering::SeqCst);
    s.text_run_id = run_id; s.text_phase = "translating".into(); s.text_original = text.into(); s.text_translation.clear(); s.text_message = "Çeviri yapılıyor…".into();
    drop(s);
    crate::document::translate::translate_document(app.clone(), DocTranslateRequest { run_id, texts: vec![text.into()], api_key: key, api_base: "https://generativelanguage.googleapis.com/v1beta/models/".into(), model, from_label: language(&source).unwrap_or("Auto").into(), to_label: language(&target).unwrap_or("Turkish").into(), chars_per_request: 6000, rpm_limit: rpm, glossary });
    Ok(json!({"ok":true}))
}
fn text_cancel(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    if state.lock().map_err(|e| e.to_string())?.text_phase != "translating" { return Err("Durdurulacak metin çevirisi yok.".into()); }
    crate::document::translate::cancel_document_translation();
    Ok(json!({"ok":true}))
}
fn text_status(state: &Arc<Mutex<State>>) -> Result<Value, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    Ok(json!({"phase":s.text_phase,"original":s.text_original,"translation":s.text_translation,"message":s.text_message}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_api_requires_token_and_rejects_foreign_origins_and_hosts() {
        let address = "127.0.0.1:49152";
        assert!(authorized(Some(address), Some("Bearer secret"), Some("http://127.0.0.1:49152"), address, "secret"));
        assert!(authorized(Some(address), Some("Bearer secret"), None, address, "secret"));
        assert!(!authorized(Some(address), None, None, address, "secret"));
        assert!(!authorized(Some(address), Some("Bearer secret"), Some("https://other.example"), address, "secret"));
        assert!(!authorized(Some("attacker.example"), Some("Bearer secret"), None, address, "secret"));
    }
    #[test]
    fn cache_keys_separate_model_language_and_text() {
        let first = cache_hash("model-a", "en", "tr", "word");
        assert_ne!(first, cache_hash("model-b", "en", "tr", "word"));
        assert_ne!(first, cache_hash("model-a", "en", "de", "word"));
        assert_ne!(first, cache_hash("model-a", "en", "tr", "other"));
    }
    #[test]
    fn identifies_valid_epub_and_rejects_other_zip_files() {
        use std::io::Write;
        let archive = |mime: &str| {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip.start_file("mimetype", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(mime.as_bytes()).unwrap();
            zip.finish().unwrap().into_inner()
        };
        assert_eq!(detect_document(b"%PDF-1.7\n"), Ok("pdf"));
        assert_eq!(detect_document(&archive("application/epub+zip")), Ok("epub"));
        assert!(detect_document(&archive("application/zip")).is_err());
        assert!(detect_document(b"not a document").is_err());
    }
    #[test]
    fn epub_keeps_commands_and_formulas_but_translates_prose() {
        assert!(epub_protected("git checkout 123"));
        assert!(epub_protected("x = 2 + 3"));
        assert!(epub_protected("12 / 63"));
        assert!(epub_protected("$$E=mc^2$$"));
        assert!(!epub_protected("图 2-1"));
        assert!(!epub_protected("A short chapter title"));
    }
}
