//! Authenticated browser transport for existing Tauri APIs. Callback IDs are remapped by the worker.
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::{collections::{HashMap, VecDeque}, sync::{atomic::{AtomicBool, Ordering}, Mutex}, time::{Duration, Instant}};
use tauri::Manager;

static READY: AtomicBool = AtomicBool::new(false);
static CLIENTS: Lazy<Mutex<HashMap<String, Client>>> = Lazy::new(|| Mutex::new(HashMap::new()));
const MAX_QUEUE_BYTES: usize = 64 * 1024 * 1024;
struct Client { seen: Instant, next: u64, pending: usize, bytes: usize, queue: VecDeque<(u64, Value, usize)> }

pub fn start(app: &tauri::AppHandle) -> Result<(), String> {
    // WebView2 requires identical browser options for a shared user-data folder.
    // The legacy daemon disables CORS; keep this worker in its own secure profile.
    let profile = app.path_resolver().app_local_data_dir().ok_or("Missing application data directory")?.join("web-worker");
    tauri::WindowBuilder::new(app, "web_bridge", tauri::WindowUrl::App("bridge.html".into()))
        .data_directory(profile).title("TurkTranslate services").visible(false).build().map_err(|e| e.to_string())?;
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        let mut expired = vec![];
        if let Ok(mut clients) = CLIENTS.lock() {
            clients.retain(|id, client| {
                let alive = client.seen.elapsed() < Duration::from_secs(45);
                if !alive { expired.push(id.clone()); }
                alive
            });
        }
        for client in expired {
            crate::web_fetch::disconnect(&client);
            let _ = app.emit_to("web_bridge", "web_bridge://disconnect", client);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn web_bridge_ready(window: tauri::Window) -> Result<(), String> {
    if window.label() != "web_bridge" { return Err("Invalid worker".into()); }
    READY.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn web_bridge_publish(window: tauri::Window, client: String, callback_id: u64, payload: Value, completed: bool) -> Result<(), String> {
    if window.label() != "web_bridge" { return Err("Invalid worker".into()); }
    let mut clients = CLIENTS.lock().map_err(|e| e.to_string())?;
    if let Some(c) = clients.get_mut(&client) {
        let event = json!({"callback": callback_id, "payload": payload});
        let bytes = event.to_string().len();
        if c.queue.len() >= 8192 || c.bytes + bytes > MAX_QUEUE_BYTES {
            clients.remove(&client);
            return Err("Browser event buffer exceeded; reconnect required".into());
        }
        c.next += 1;
        c.bytes += bytes;
        if completed { c.pending = c.pending.saturating_sub(1); }
        c.queue.push_back((c.next, event, bytes));
    }
    Ok(())
}

pub fn connect() -> Result<Value, String> {
    if !READY.load(Ordering::SeqCst) { return Err("Servisler hazırlanıyor…".into()); }
    let mut clients = CLIENTS.lock().map_err(|e| e.to_string())?;
    if clients.len() >= 8 { return Err("En fazla sekiz tarayıcı sekmesi açılabilir.".into()); }
    let id = uuid::Uuid::new_v4().to_string();
    clients.insert(id.clone(), Client { seen: Instant::now(), next: 0, pending: 0, bytes: 0, queue: VecDeque::new() });
    Ok(json!({"client": id}))
}

fn allowed(message: &Value) -> bool {
    let cmd = message["cmd"].as_str().unwrap_or("");
    if cmd == "tauri" {
        return match message["__tauriModule"].as_str().unwrap_or("") {
            "App" | "Os" | "Path" | "Fs" | "Dialog" | "Http" | "Clipboard" | "Notification" => true,
            "Shell" => message["message"]["cmd"] == "open",
            "GlobalShortcut" => matches!(message["message"]["cmd"].as_str(), Some("unregister" | "isRegistered")),
            "Event" => {
                let event = message["message"]["event"].as_str().unwrap_or("");
                !event.starts_with("web_bridge:") && matches!(message["message"]["cmd"].as_str(), Some("listen" | "unlisten" | "emit"))
            },
            "Window" => message["message"]["data"]["label"] == "live_caption" && matches!(message["message"]["data"]["cmd"]["type"].as_str(), Some("setIgnoreCursorEvents" | "close" | "show" | "hide")),
            _ => false,
        };
    }
    if let Some((plugin, operation)) = cmd.strip_prefix("plugin:").and_then(|s| s.split_once('|')) {
        return match plugin {
            "store" => ["load", "get", "set", "has", "delete", "clear", "reset", "keys", "values", "entries", "length", "save"].contains(&operation),
            "sql" => ["load", "execute", "select", "close"].contains(&operation),
            "fs-watch" => ["watch", "unwatch"].contains(&operation),
            "autostart" => ["enable", "disable", "is_enabled"].contains(&operation),
            "log" => operation == "log",
            _ => false,
        };
    }
    ["get_text", "get_base64", "copy_img", "reload_store", "system_ocr", "lang_detect", "set_proxy", "unset_proxy",
     "run_binary", "register_shortcut_by_frontend", "update_tray", "webdav", "local", "aliyun", "install_plugin", "font_list",
     "get_active_glossary", "add_glossary_entry", "update_glossary_entry", "delete_glossary_entry", "list_glossaries",
     "extract_pdf_pages", "extract_pdf_layout", "render_pdf_page", "export_translated_pdf", "export_layout_pdf", "cancel_layout_export",
     "submit_layout_image_ocr", "extract_pdf_images", "scan_pdf_images", "check_image_service", "translate_image_via_service",
     "extract_epub", "assemble_epub", "translate_document", "cancel_document_translation", "transcribe_media", "cancel_transcription",
     "start_live_caption", "stop_live_caption", "live_caption_window", "cache_get_translation", "cache_set_translation", "cache_stats", "cache_clear",
     "web_desktop_action", "web_import_image", "web_fetch_start", "web_fetch_cancel"].contains(&cmd)
}

pub fn submit(app: &tauri::AppHandle, client: &str, message: Value) -> Result<Value, String> {
    if !allowed(&message) || message["callback"].as_u64().is_none() || message["error"].as_u64().is_none() {
        return Err("Bu işlem web oturumunda desteklenmiyor.".into());
    }
    {
        let mut clients = CLIENTS.lock().map_err(|e| e.to_string())?;
        let c = clients.get_mut(client).ok_or("Tarayıcı oturumu sona erdi. Sayfayı yenileyin.")?;
        if c.pending >= 256 { return Err("Çok fazla eşzamanlı işlem.".into()); }
        c.pending += 1;
        c.seen = Instant::now();
    }
    app.emit_to("web_bridge", "web_bridge://request", json!({"client":client,"message":message})).map_err(|e| e.to_string())?;
    Ok(json!({"ok":true}))
}

pub fn poll(client: &str, after: u64) -> Result<Value, String> {
    let mut clients = CLIENTS.lock().map_err(|e| e.to_string())?;
    let c = clients.get_mut(client).ok_or("Tarayıcı oturumu sona erdi. Sayfayı yenileyin.")?;
    c.seen = Instant::now();
    while c.queue.front().is_some_and(|(id, _, _)| *id <= after) {
        if let Some((_, _, size)) = c.queue.pop_front() { c.bytes -= size; }
    }
    let events: Vec<_> = c.queue.iter().take(256).map(|(id, event, _)| json!({"id":id,"event":event})).collect();
    Ok(json!({"events":events}))
}

pub fn navigate(page: &str, event: &str, payload: &str) -> bool {
    let active = CLIENTS.lock().map(|clients| clients.values().any(|c| c.seen.elapsed() < Duration::from_secs(10))).unwrap_or(false);
    if active {
        if let Some(app) = crate::APP.get() { let _ = app.emit_all("web:navigate", json!({"page":page,"event":event,"payload":payload})); }
    }
    active
}

#[tauri::command]
pub fn web_desktop_action(action: String) -> Result<(), String> {
    let app = crate::APP.get().ok_or("Application not ready")?;
    if !["ocr_recognize", "ocr_translate", "selection_translate", "input_translate"].contains(&action.as_str()) { return Err("Unknown action".into()); }
    app.run_on_main_thread(move || match action.as_str() {
        "ocr_recognize" => crate::window::ocr_recognize(),
        "ocr_translate" => crate::window::ocr_translate(),
        "selection_translate" => crate::window::selection_translate(),
        _ => crate::window::input_translate(),
    }).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn web_import_image(app: tauri::AppHandle, data: Vec<u8>) -> Result<(), String> {
    if data.len() > 25 * 1024 * 1024 { return Err("Görsel en fazla 25 MB olabilir.".into()); }
    let image = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().map_err(|e| e.to_string())?.decode().map_err(|e| e.to_string())?;
    let dir = app.path_resolver().app_cache_dir().ok_or("Cache directory missing")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    image.save(dir.join("pot_screenshot_cut.png")).map_err(|e| e.to_string())?;
    app.emit_all("new_image", "").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_rejects_native_escape_hatches() {
        for cmd in ["web_bridge_publish", "web_bridge_ready", "open_devtools", "plugin:unknown|run"] { assert!(!allowed(&json!({"cmd":cmd}))); }
        assert!(!allowed(&json!({"cmd":"tauri","__tauriModule":"Shell","message":{"cmd":"execute"}})));
        assert!(!allowed(&json!({"cmd":"tauri","__tauriModule":"Event","message":{"cmd":"emit","event":"web_bridge://request"}})));
        assert!(allowed(&json!({"cmd":"plugin:store|get"})));
        assert!(allowed(&json!({"cmd":"translate_document"})));
    }
    #[test]
    fn event_poll_replays_until_acknowledged_and_isolates_clients() {
        let id = uuid::Uuid::new_v4().to_string();
        CLIENTS.lock().unwrap().insert(id.clone(), Client { seen: Instant::now(), next: 1, pending: 0, bytes: 2, queue: VecDeque::from([(1,json!({"callback":7,"payload":"ok"}),2)]) });
        assert_eq!(poll(&id,0).unwrap()["events"].as_array().unwrap().len(),1);
        assert_eq!(poll(&id,0).unwrap()["events"].as_array().unwrap().len(),1);
        assert!(poll("not-this-client",0).is_err());
        assert_eq!(poll(&id,1).unwrap()["events"].as_array().unwrap().len(),0);
        CLIENTS.lock().unwrap().remove(&id);
    }
}
