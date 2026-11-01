//! Streaming HTTP for browser engines. Uses the same local network/proxy as the
//! desktop and keeps provider credentials out of URLs and browser history.
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tauri::Manager;

static REQUESTS: Lazy<Mutex<HashMap<String, (String, tauri::async_runtime::JoinHandle<()>)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Deserialize)]
pub struct FetchRequest {
    id: String,
    url: String,
    method: String,
    headers: HashMap<String, String>,
    body: Option<Vec<u8>>,
}

#[tauri::command]
pub fn web_fetch_start(app: tauri::AppHandle, client: String, request: FetchRequest) -> Result<(), String> {
    uuid::Uuid::parse_str(&request.id).map_err(|_| "Invalid request ID")?;
    let url = reqwest::Url::parse(&request.url).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") { return Err("HTTP(S) required".into()); }
    if request.body.as_ref().is_some_and(|body| body.len() > 32 * 1024 * 1024) { return Err("Request body exceeds 32 MB".into()); }
    let mut pending = REQUESTS.lock().map_err(|e| e.to_string())?;
    if pending.len() >= 32 || pending.contains_key(&request.id) { return Err("Too many active network requests".into()); }
    let id = request.id.clone();
    let task = tauri::async_runtime::spawn(async move {
        let event = format!("web_fetch://{}", request.id);
        let result: Result<(), String> = async {
            let http = reqwest::Client::builder().timeout(Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
            let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|e| e.to_string())?;
            let mut builder = http.request(method, url);
            for (name, value) in request.headers { builder = builder.header(name, value); }
            if let Some(body) = request.body { builder = builder.body(body); }
            let mut response = builder.send().await.map_err(|e| e.without_url().to_string())?;
            let headers: HashMap<_, _> = response.headers().iter().filter_map(|(key, value)| value.to_str().ok().map(|v| (key.as_str().to_string(), v.to_string()))).collect();
            app.emit_all(&event, json!({"type":"headers","status":response.status().as_u16(),"headers":headers})).map_err(|e| e.to_string())?;
            let mut bytes = 0usize;
            while let Some(chunk) = response.chunk().await.map_err(|e| e.without_url().to_string())? {
                bytes += chunk.len();
                if bytes > 64 * 1024 * 1024 { return Err("Response exceeds 64 MB".into()); }
                for part in chunk.chunks(16 * 1024) {
                    app.emit_all(&event, json!({"type":"chunk","data":part})).map_err(|e| e.to_string())?;
                }
            }
            app.emit_all(&event, json!({"type":"done"})).map_err(|e| e.to_string())?;
            Ok(())
        }.await;
        if let Err(error) = result { let _ = app.emit_all(&event, json!({"type":"error","error":error})); }
    });
    pending.insert(id, (client, task));
    Ok(())
}

#[tauri::command]
pub fn web_fetch_cancel(client: String, id: String) -> Result<(), String> {
    let mut pending = REQUESTS.lock().map_err(|e| e.to_string())?;
    if pending.get(&id).is_some_and(|(owner, _)| owner != &client) { return Err("Request belongs to another session".into()); }
    if let Some((_, task)) = pending.remove(&id) { task.abort(); }
    Ok(())
}

pub fn disconnect(client: &str) {
    if let Ok(mut requests) = REQUESTS.lock() {
        requests.retain(|_, (owner, task)| { if owner == client { task.abort(); false } else { true } });
    }
}
