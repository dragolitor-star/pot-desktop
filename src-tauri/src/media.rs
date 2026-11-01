// Phase 4 — Audio/Video → subtitle transcription (HANDOFF §1 table / Phase 4).
//
// FILE-based only (system-audio capture is OUT of scope per HANDOFF §2). The
// project is Gemini-first, so transcription is done with Gemini's audio/video
// understanding: the media file is uploaded once via the Gemini File API and a
// single `generateContent` call returns schema-constrained subtitle cues
// (`[{ start, end, text }]`). Mirrors `document::translate` (background thread +
// Tauri progress events + run-supersede/cancel) so the renderer stays responsive
// and cancellation is instant.
//
// What stays frontend-owned (single-sourced, exactly like the document flow):
//   * subtitle translation (reuses the existing `translate_document` pipeline),
//   * SRT/VTT formatting + file export (frontend `utils/subtitle.js`).
// This module ONLY turns a media file into normalized cues and streams them back.
//
// Events emitted to the `subtitle` window (all carry `runId` so stale runs are
// ignored):
//   media://progress  { runId, percent, text }
//   media://cues      { runId, cues: [{ start, end, text }] }   // seconds as f64
//   media://done      { runId, status: "completed"|"rate_limited"|"cancelled"|"error",
//                       message, count }

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tauri::Manager;

// Live (real-time) captions from system-audio loopback (Task B). Strictly additive
// submodule — the file-based transcription above is untouched. (Rust 2018 allows a
// `media.rs` module to own a `media/` submodule directory.)
pub mod live_caption;

// Only one transcription runs at a time. A newer run (different id) or an explicit
// cancel makes the older thread stop at the next checkpoint.
static ACTIVE_RUN: AtomicU64 = AtomicU64::new(0);
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

const EVT_PROGRESS: &str = "media://progress";
const EVT_CUES: &str = "media://cues";
const EVT_DONE: &str = "media://done";

const MAX_OUTPUT_TOKENS: u32 = 32768;
// Hard ceiling on a single media file. The Gemini File API accepts up to 2 GB per
// file; we stream the upload in chunks straight from disk (see `upload_file`) so even
// multi-hundred-MB media never sits fully in memory.
const MAX_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
// Resumable-upload chunk size. Per the Google resumable-upload protocol every chunk
// except the last must be a multiple of 256 KiB; 8 MiB balances request count vs RAM.
const UPLOAD_CHUNK_BYTES: u64 = 8 * 1024 * 1024;
// Safety cap on transcription continuation rounds for very long media (each round
// resumes after the previous tail when the model hits MAX_TOKENS). Guarantees the
// loop always terminates while still covering hours of speech.
const MAX_TRANSCRIBE_ROUNDS: u32 = 12;
// How long to wait for the uploaded file to leave PROCESSING and become ACTIVE.
const FILE_ACTIVE_POLL_ATTEMPTS: u32 = 60;
const FILE_ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeRequest {
    pub run_id: u64,
    /// Absolute path to the local audio/video file.
    pub path: String,
    pub api_key: String,
    /// API base URL, already resolved by the frontend (ends with `models/`).
    pub api_base: String,
    pub model: String,
    /// Spoken-language hint (e.g. "English"); empty/"Auto" lets the model detect.
    #[serde(default)]
    pub language_hint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct Cue {
    /// Start time in seconds.
    start: f64,
    /// End time in seconds.
    end: f64,
    text: String,
}

/// Kick off the background transcription and return immediately. Progress and
/// cues are delivered through the events documented above.
#[tauri::command]
pub fn transcribe_media(app_handle: tauri::AppHandle, request: TranscribeRequest) {
    let run_id = request.run_id;
    ACTIVE_RUN.store(run_id, Ordering::SeqCst);
    CANCEL_FLAG.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        run_transcription(app_handle, request);
    });
}

/// Request cancellation of the currently-running transcription.
#[tauri::command]
pub fn cancel_transcription() {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
}

/// True when this run should stop — explicit cancel, or a newer run superseded it.
fn should_stop(run_id: u64) -> bool {
    CANCEL_FLAG.load(Ordering::SeqCst) || ACTIVE_RUN.load(Ordering::SeqCst) != run_id
}

fn run_transcription(app: tauri::AppHandle, req: TranscribeRequest) {
    let run_id = req.run_id;

    let mime = match guess_media_mime(&req.path) {
        Some(m) => m.to_string(),
        None => {
            emit_done(
                &app,
                run_id,
                "error",
                "Unsupported media type. Use a common audio/video file (mp3, wav, m4a, aac, ogg, flac, mp4, mov, mkv, webm, …).",
                0,
            );
            return;
        }
    };

    let total_len = match media_len_capped(&req.path) {
        Ok(l) => l,
        Err(e) => {
            emit_done(&app, run_id, "error", &e, 0);
            return;
        }
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            emit_done(&app, run_id, "error", &format!("HTTP client init failed: {e}"), 0);
            return;
        }
    };

    if should_stop(run_id) {
        emit_done(&app, run_id, "cancelled", "Transcription cancelled.", 0);
        return;
    }

    // 1) Upload the file (Gemini File API, resumable single-shot).
    emit_progress(&app, run_id, 10, "Uploading media to Gemini…");
    let file_name = std::path::Path::new(&req.path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".to_string());
    let (upload_url, files_base) = derive_file_api_urls(&req.api_base);

    let file_uri = match upload_file(&client, &app, &upload_url, &files_base, &req.api_key, &req.path, &file_name, &mime, total_len, run_id) {
        Ok(uri) => uri,
        Err(err) => {
            if err.rate_limited {
                emit_done(&app, run_id, "rate_limited", &err.message, 0);
            } else if err.message == "cancelled" {
                emit_done(&app, run_id, "cancelled", "Transcription cancelled.", 0);
            } else {
                emit_done(&app, run_id, "error", &err.message, 0);
            }
            return;
        }
    };

    if should_stop(run_id) {
        emit_done(&app, run_id, "cancelled", "Transcription cancelled.", 0);
        return;
    }

    // 2) Ask Gemini for schema-constrained subtitle cues. A single call can hit
    // MAX_TOKENS and truncate the tail on long media; we then CONTINUE from the last
    // timestamp against the SAME uploaded file (no re-upload, no local audio
    // splitting) until the model stops making forward progress or yields no more
    // speech. This keeps us inside the no-heavy-dependency rule (no ffmpeg).
    emit_progress(&app, run_id, 55, "Transcribing audio (this can take a while for long media)…");
    let url = format!("{}{}:generateContent", req.api_base, req.model);

    let mut all: Vec<Cue> = Vec::new();
    let mut resume_after: Option<f64> = None;
    let mut rounds = 0u32;

    loop {
        if should_stop(run_id) {
            emit_done(&app, run_id, "cancelled", "Transcription cancelled.", all.len());
            return;
        }
        rounds += 1;
        let body = build_transcribe_body(&req, &mime, &file_uri, resume_after);
        match transcribe_once(&client, &url, &body, &req.api_key) {
            Ok(part) => {
                // Drop anything the model repeated from before the resume point, then
                // merge, order and stream the cumulative cues so the UI fills live.
                let min_start = resume_after.map(|r| r - 0.001).unwrap_or(f64::MIN);
                let mut added = 0usize;
                for c in part.cues {
                    if c.start < min_start {
                        continue;
                    }
                    all.push(c);
                    added += 1;
                }
                sort_dedup_clamp(&mut all);
                emit_cues(&app, run_id, &all);

                let last_end = all.iter().map(|c| c.end).fold(0.0f64, f64::max);
                let advanced = resume_after.map_or(true, |r| last_end > r + 0.5);

                // Continue only while the model is truncating AND still moving forward.
                if part.truncated && added > 0 && advanced && rounds < MAX_TRANSCRIBE_ROUNDS {
                    resume_after = Some(last_end);
                    let pct = (55 + rounds * 4).min(92);
                    emit_progress(
                        &app,
                        run_id,
                        pct,
                        &format!("Transcribing long media — continuing past {}…", fmt_timestamp_hms(last_end)),
                    );
                    continue;
                }
                break;
            }
            Err(err) => {
                if err.rate_limited {
                    emit_done(&app, run_id, "rate_limited", &err.message, all.len());
                    return;
                }
                // A failure on a continuation round keeps the segments collected so far
                // rather than discarding the whole run; a first-round failure is fatal.
                if all.is_empty() {
                    emit_done(&app, run_id, "error", &err.message, 0);
                    return;
                }
                break;
            }
        }
    }

    if all.is_empty() {
        emit_done(&app, run_id, "error", "The model returned no usable subtitle segments.", 0);
        return;
    }
    emit_progress(&app, run_id, 95, "Finalizing subtitle segments…");
    emit_cues(&app, run_id, &all);
    emit_done(&app, run_id, "completed", "Transcription completed.", all.len());
}

// ── file reading ──────────────────────────────────────────────────────────--

fn media_len_capped(path: &str) -> Result<u64, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Cannot open media file: {e}"))?;
    let len = meta.len();
    if len == 0 {
        return Err("Media file is empty.".into());
    }
    if len > MAX_MEDIA_BYTES {
        return Err(format!(
            "Media file is {:.2} GB; the Gemini File API limit is {} GB. Trim or compress the clip.",
            len as f64 / (1024.0 * 1024.0 * 1024.0),
            MAX_MEDIA_BYTES / (1024 * 1024 * 1024)
        ));
    }
    Ok(len)
}

// ── MIME detection ──────────────────────────────────────────────────────────

/// Map a file extension to a Gemini-accepted audio/video MIME type. Returns None
/// for anything that isn't a recognized media container so we never upload junk.
fn guess_media_mime(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let mime = match ext.as_str() {
        // audio
        "mp3" => "audio/mp3",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "aiff" | "aif" => "audio/aiff",
        "opus" => "audio/opus",
        // video (Gemini extracts the audio track for transcription)
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mpeg" | "mpg" => "video/mpeg",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "3gp" => "video/3gpp",
        _ => return None,
    };
    Some(mime)
}

// ── Gemini File API ───────────────────────────────────────────────────────--

struct ApiError {
    message: String,
    rate_limited: bool,
}

fn is_rate_limited(status: u16, body: &str) -> bool {
    if status == 429 {
        return true;
    }
    let b = body.to_lowercase();
    b.contains("too many requests")
        || b.contains("rate limit")
        || b.contains("quota")
        || b.contains("resource exhausted")
        || b.contains("resource_exhausted")
}

/// Derive the File API upload + files endpoints from the resolved generateContent
/// base (which ends with `…/v1beta/models/`). The upload endpoint mirrors the
/// version path prefixed with `upload/`, the files endpoint drops `models`.
/// e.g. `https://host/v1beta/models/` →
///   upload: `https://host/upload/v1beta/files`
///   files : `https://host/v1beta/files`
fn derive_file_api_urls(api_base: &str) -> (String, String) {
    let mut v = api_base.trim_end_matches('/');
    if let Some(stripped) = v.strip_suffix("models") {
        v = stripped.trim_end_matches('/');
    }
    let version_base = v.to_string(); // …/v1beta
    let files_base = format!("{}/files", version_base);
    let upload_url = match version_base.rfind('/') {
        // Only insert before a real path segment (after the scheme's "//").
        Some(idx) if idx > version_base.find("//").map(|i| i + 1).unwrap_or(0) => {
            format!("{}/upload/{}/files", &version_base[..idx], &version_base[idx + 1..])
        }
        _ => format!("{}/upload/files", version_base),
    };
    (upload_url, files_base)
}

#[allow(clippy::too_many_arguments)]
fn upload_file(
    client: &reqwest::blocking::Client,
    app: &tauri::AppHandle,
    upload_url: &str,
    files_base: &str,
    api_key: &str,
    path: &str,
    display_name: &str,
    mime: &str,
    total_len: u64,
    run_id: u64,
) -> Result<String, ApiError> {
    let start_url = upload_url;

    // 1) Start a resumable session.
    let start = client
        .post(start_url)
        .header("x-goog-api-key", api_key)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header("X-Goog-Upload-Header-Content-Length", total_len.to_string())
        .header("X-Goog-Upload-Header-Content-Type", mime)
        .header("Content-Type", "application/json")
        .body(json!({ "file": { "display_name": display_name } }).to_string())
        .send()
        .map_err(|e| ApiError { message: format!("Upload start failed: {e}"), rate_limited: false })?;

    let start_status = start.status().as_u16();
    let session_url = start
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if start_status < 200 || start_status >= 300 || session_url.is_none() {
        let body = start.text().unwrap_or_default();
        return Err(ApiError {
            message: format!("Upload start rejected (HTTP {start_status}). {body}"),
            rate_limited: is_rate_limited(start_status, &body),
        });
    }
    let session_url = session_url.unwrap();

    // 2) Stream the file from disk in chunks. Every chunk but the last is a fixed
    // multiple of 256 KiB; the final chunk carries the `finalize` command and its
    // response holds the file resource. This keeps even multi-GB media off the heap.
    let mut file = std::fs::File::open(path)
        .map_err(|e| ApiError { message: format!("Cannot open media file: {e}"), rate_limited: false })?;
    let mut offset: u64 = 0;
    let final_text;
    loop {
        if should_stop(run_id) {
            return Err(ApiError { message: "cancelled".into(), rate_limited: false });
        }
        let this_len = (total_len - offset).min(UPLOAD_CHUNK_BYTES);
        let mut buf = vec![0u8; this_len as usize];
        file.read_exact(&mut buf)
            .map_err(|e| ApiError { message: format!("Failed to read media file: {e}"), rate_limited: false })?;
        let is_last = offset + this_len >= total_len;
        let command = if is_last { "upload, finalize" } else { "upload" };

        let resp = client
            .post(&session_url)
            .header("Content-Length", this_len.to_string())
            .header("X-Goog-Upload-Offset", offset.to_string())
            .header("X-Goog-Upload-Command", command)
            .body(buf)
            .send()
            .map_err(|e| ApiError { message: format!("Upload failed: {}", e.without_url()), rate_limited: false })?;

        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        // Gemini's resumable upload returns 2xx for an accepted intermediate chunk
        // (some proxies use 308 Resume Incomplete); only finalize returns the file JSON.
        let ok = (200..300).contains(&status) || (!is_last && status == 308);
        if !ok {
            return Err(ApiError {
                message: format!("Upload chunk rejected (HTTP {status}). {text}"),
                rate_limited: is_rate_limited(status, &text),
            });
        }

        offset += this_len;
        if is_last {
            final_text = text;
            break;
        }
        if total_len > 0 {
            let pct = 10 + ((offset as f64 / total_len as f64) * 40.0) as u32;
            emit_progress(app, run_id, pct.min(50), "Uploading media to Gemini…");
        }
    }

    let payload: Value = serde_json::from_str(&final_text)
        .map_err(|e| ApiError { message: format!("Invalid upload response: {e}"), rate_limited: false })?;
    let file_json = payload.get("file").unwrap_or(&payload);
    let mut uri = file_json.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let name = file_json.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut state = file_json.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if uri.is_empty() {
        return Err(ApiError { message: format!("Upload response missing file uri: {final_text}"), rate_limited: false });
    }

    // 3) Audio/video files start in PROCESSING; wait until ACTIVE before referencing.
    let get_url = format!("{}/{}", files_base, name.trim_start_matches("files/").trim());
    let mut attempts = 0u32;
    while state == "PROCESSING" {
        if should_stop(run_id) {
            return Err(ApiError { message: "cancelled".into(), rate_limited: false });
        }
        if attempts >= FILE_ACTIVE_POLL_ATTEMPTS {
            return Err(ApiError {
                message: "Timed out waiting for Gemini to finish processing the uploaded media.".into(),
                rate_limited: false,
            });
        }
        std::thread::sleep(FILE_ACTIVE_POLL_INTERVAL);
        attempts += 1;
        if name.is_empty() {
            break; // can't poll without a name; assume the uri is usable
        }
        if let Ok(r) = client.get(&get_url).header("x-goog-api-key", api_key).send() {
            let body = r.text().unwrap_or_default();
            if let Ok(p) = serde_json::from_str::<Value>(&body) {
                let f = p.get("file").unwrap_or(&p);
                if let Some(s) = f.get("state").and_then(|v| v.as_str()) {
                    state = s.to_string();
                }
                if let Some(u) = f.get("uri").and_then(|v| v.as_str()) {
                    if !u.is_empty() {
                        uri = u.to_string();
                    }
                }
            }
        }
    }
    if state == "FAILED" {
        return Err(ApiError {
            message: "Gemini could not process the uploaded media (state FAILED).".into(),
            rate_limited: false,
        });
    }

    Ok(uri)
}

// ── transcription request + parse ─────────────────────────────────────────--

fn thinking_config_for(model: &str) -> Option<Value> {
    let m = model.to_lowercase();
    let is_thinking = m.contains("gemini-2.5") || m.contains("gemini-3");
    if !is_thinking {
        return None;
    }
    if m.contains("pro") {
        Some(json!({ "thinkingBudget": 128 }))
    } else {
        Some(json!({ "thinkingBudget": 0 }))
    }
}

fn build_transcribe_body(req: &TranscribeRequest, mime: &str, file_uri: &str, resume_after: Option<f64>) -> Value {
    let lang_clause = if !req.language_hint.is_empty() && req.language_hint != "Auto" {
        format!("The spoken language is {}. ", req.language_hint)
    } else {
        String::new()
    };

    // On a continuation pass, ask the model to resume after the last timestamp we
    // already have (against the same uploaded file) so long media isn't truncated.
    let resume_clause = match resume_after {
        Some(t) if t > 0.0 => format!(
            "IMPORTANT: A previous pass already transcribed everything up to {ts}. Continue \
transcribing ONLY the audio AFTER {ts}; do NOT repeat any earlier segment. Keep timestamps \
relative to the START of the media (the first new segment should begin around {ts}). If there \
is no more speech after {ts}, return an empty array []. ",
            ts = fmt_timestamp_hms(t)
        ),
        _ => String::new(),
    };

    let instruction = format!(
        "{lang_clause}{resume_clause}Transcribe the speech in this media into time-coded subtitle \
segments. Return one item per segment with its start time, end time and the spoken \
text in the ORIGINAL language — do NOT translate. Keep each segment short (a single \
sentence or natural phrase, roughly 3–10 seconds). Timestamps must be relative to the \
start of the media in HH:MM:SS.mmm format. Go in order, never overlap segments, and do \
not add commentary or notes."
    );

    let mut generation_config = json!({
        "maxOutputTokens": MAX_OUTPUT_TOKENS,
        "responseMimeType": "application/json",
        "responseSchema": {
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "start": { "type": "STRING" },
                    "end": { "type": "STRING" },
                    "text": { "type": "STRING" }
                },
                "required": ["start", "end", "text"],
                "propertyOrdering": ["start", "end", "text"]
            }
        }
    });
    if let Some(tc) = thinking_config_for(&req.model) {
        generation_config["thinkingConfig"] = tc;
    }

    json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": instruction },
                { "file_data": { "mime_type": mime, "file_uri": file_uri } }
            ]
        }],
        "safetySettings": [
            { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE" }
        ],
        "generationConfig": generation_config
    })
}

struct TranscribePartial {
    cues: Vec<Cue>,
    /// True when the model hit MAX_TOKENS — the caller continues from the last cue.
    truncated: bool,
}

fn transcribe_once(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &Value,
    api_key: &str,
) -> Result<TranscribePartial, ApiError> {
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("x-goog-api-key", api_key)
        .json(body)
        .send()
        .map_err(|e| ApiError { message: format!("Network error: {e}"), rate_limited: false })?;

    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status < 200 || status >= 300 {
        return Err(ApiError {
            message: format!("Http Request Error\nHttp Status: {status}\n{text}"),
            rate_limited: is_rate_limited(status, &text),
        });
    }

    let payload: Value = serde_json::from_str(&text)
        .map_err(|e| ApiError { message: format!("Invalid JSON from API: {e}"), rate_limited: false })?;
    let cand = payload
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .ok_or_else(|| ApiError {
            message: format!("No candidates in response: {text}"),
            rate_limited: is_rate_limited(status, &text),
        })?;

    let mut truncated = false;
    if let Some(finish) = cand.get("finishReason").and_then(|f| f.as_str()) {
        if finish == "MAX_TOKENS" {
            truncated = true;
        } else if finish != "STOP" {
            return Err(ApiError { message: format!("Gemini stopped: finishReason={finish}"), rate_limited: false });
        }
    }

    let raw = cand
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| ApiError { message: format!("No text part in candidate: {text}"), rate_limited: false })?;

    // Empty cues are NOT an error here — a continuation round legitimately returns
    // `[]` when the media has ended; the caller decides whether that is fatal.
    Ok(TranscribePartial { cues: parse_cues(raw), truncated })
}

/// Parse the model's `[{ start, end, text }]` JSON into normalized, ordered cues.
/// Defensive against a MAX_TOKENS-truncated tail (salvages complete objects), bad
/// timestamps (dropped), and non-monotonic/zero-length spans (clamped) so a partial
/// response still yields every finished segment.
fn parse_cues(raw: &str) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    for el in parse_structured_array(raw) {
        let text = el.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if text.is_empty() {
            continue;
        }
        let start = el.get("start").and_then(parse_ts_value);
        let end = el.get("end").and_then(parse_ts_value);
        let start = match start {
            Some(s) if s.is_finite() && s >= 0.0 => s,
            _ => continue,
        };
        // A missing/invalid end is tolerated: give the cue a small default span so
        // it still shows and exports. End before start is clamped forward.
        let end = match end {
            Some(e) if e.is_finite() && e > start => e,
            _ => start + 2.0,
        };
        cues.push(Cue { start, end, text });
    }
    cues
}

fn parse_ts_value(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    v.as_str().and_then(parse_timestamp)
}

/// Parse a timestamp into seconds. Accepts plain seconds ("12.5"), `MM:SS(.mmm)`,
/// `HH:MM:SS(.mmm)` and the SRT comma-decimal variant (`00:01:02,500`).
fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    if !s.contains(':') {
        return s.parse::<f64>().ok().filter(|n| n.is_finite());
    }
    let mut total = 0.0f64;
    for part in s.split(':') {
        let val = part.parse::<f64>().ok()?;
        if !val.is_finite() {
            return None;
        }
        total = total * 60.0 + val;
    }
    Some(total)
}

/// Format seconds as HH:MM:SS.mmm for the resume-instruction prompt and progress text.
fn fmt_timestamp_hms(seconds: f64) -> String {
    let s = if seconds.is_finite() && seconds > 0.0 { seconds } else { 0.0 };
    let total_ms = (s * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        total_secs / 3600,
        (total_secs % 3600) / 60,
        total_secs % 60,
        ms
    )
}

/// Order cues by start, drop exact duplicates (same start+text — a continuation round
/// can re-emit a boundary cue), and guarantee every span is positive.
fn sort_dedup_clamp(cues: &mut Vec<Cue>) {
    cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    cues.dedup_by(|a, b| (a.start - b.start).abs() < 0.001 && a.text == b.text);
    for c in cues.iter_mut() {
        if !(c.end > c.start) {
            c.end = c.start + 2.0;
        }
    }
}

fn parse_structured_array(raw: &str) -> Vec<Value> {
    if let Ok(Value::Array(v)) = serde_json::from_str::<Value>(raw) {
        return v;
    }
    if let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) {
        if end > start {
            if let Ok(Value::Array(v)) = serde_json::from_str::<Value>(&raw[start..=end]) {
                return v;
            }
        }
    }
    salvage_objects(raw)
}

/// String-aware brace matcher — collects each balanced top-level `{…}` object and
/// parses it, so a MAX_TOKENS-truncated array still yields its finished cues. Brace
/// characters inside JSON strings are ignored.
fn salvage_objects(raw: &str) -> Vec<Value> {
    let mut objects = Vec::new();
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&raw[start..=i]) {
                            objects.push(v);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    objects
}

// ── event emit helpers ────────────────────────────────────────────────────--

fn emit_progress(app: &tauri::AppHandle, run_id: u64, percent: u32, text: &str) {
    let payload = json!({ "runId": run_id, "percent": percent, "text": text });
    app.trigger_global(EVT_PROGRESS, Some(payload.to_string()));
    let _ = app.emit_all(EVT_PROGRESS, payload);
}

fn emit_cues(app: &tauri::AppHandle, run_id: u64, cues: &[Cue]) {
    let payload = json!({ "runId": run_id, "cues": cues });
    app.trigger_global(EVT_CUES, Some(payload.to_string()));
    let _ = app.emit_all(EVT_CUES, payload);
}

fn emit_done(app: &tauri::AppHandle, run_id: u64, status: &str, message: &str, count: usize) {
    let payload = json!({ "runId": run_id, "status": status, "message": message, "count": count });
    app.trigger_global(EVT_DONE, Some(payload.to_string()));
    let _ = app.emit_all(EVT_DONE, payload);
}

// ── tests ──────────────────────────────────────────────────────────────────-

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_detection_audio_and_video() {
        assert_eq!(guess_media_mime("a/b/song.MP3"), Some("audio/mp3"));
        assert_eq!(guess_media_mime("clip.m4a"), Some("audio/mp4"));
        assert_eq!(guess_media_mime("movie.mp4"), Some("video/mp4"));
        assert_eq!(guess_media_mime("rec.MKV"), Some("video/x-matroska"));
        assert_eq!(guess_media_mime("notes.txt"), None);
        assert_eq!(guess_media_mime("noext"), None);
    }

    #[test]
    fn file_api_urls_from_default_base() {
        let (upload, files) =
            derive_file_api_urls("https://generativelanguage.googleapis.com/v1beta/models/");
        assert_eq!(upload, "https://generativelanguage.googleapis.com/upload/v1beta/files");
        assert_eq!(files, "https://generativelanguage.googleapis.com/v1beta/files");
    }

    #[test]
    fn file_api_urls_tolerate_missing_trailing_models() {
        let (upload, files) =
            derive_file_api_urls("https://proxy.example.com/v1beta/");
        assert_eq!(upload, "https://proxy.example.com/upload/v1beta/files");
        assert_eq!(files, "https://proxy.example.com/v1beta/files");
    }

    #[test]
    fn timestamp_parsing_formats() {
        assert_eq!(parse_timestamp("12.5"), Some(12.5));
        assert_eq!(parse_timestamp("01:02"), Some(62.0));
        assert_eq!(parse_timestamp("00:01:02"), Some(62.0));
        assert_eq!(parse_timestamp("01:00:00"), Some(3600.0));
        assert_eq!(parse_timestamp("00:01:02,500"), Some(62.5));
        assert_eq!(parse_timestamp("00:00:01.250"), Some(1.25));
        assert_eq!(parse_timestamp(""), None);
        assert_eq!(parse_timestamp("ab:cd"), None);
    }

    #[test]
    fn parse_cues_clean_array() {
        let raw = r#"[{"start":"00:00:01.000","end":"00:00:03.000","text":"Hello"},
                      {"start":"3","end":"5.5","text":"world"}]"#;
        let cues = parse_cues(raw);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0], Cue { start: 1.0, end: 3.0, text: "Hello".into() });
        assert_eq!(cues[1], Cue { start: 3.0, end: 5.5, text: "world".into() });
    }

    #[test]
    fn parse_cues_salvages_truncated_tail() {
        // MAX_TOKENS clip: trailing `]` and last item missing.
        let raw = r#"[{"start":"0","end":"1","text":"bir"},{"start":"1","end":"2","text":"iki"},{"start":"2","#;
        let cues = parse_cues(raw);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].text, "iki");
    }

    #[test]
    fn parse_cues_drops_empty_text_and_bad_start() {
        let raw = r#"[{"start":"0","end":"1","text":"  "},
                      {"start":"x","end":"2","text":"no start"},
                      {"start":"5","end":"4","text":"end clamps forward"}]"#;
        let cues = parse_cues(raw);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, 5.0);
        assert!(cues[0].end > cues[0].start, "non-monotonic end must be clamped forward");
    }

    #[test]
    fn salvage_handles_braces_inside_strings() {
        let raw = r#"[{"start":"0","end":"1","text":"f(x) = {a}"}]"#;
        let cues = parse_cues(raw);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "f(x) = {a}");
    }

    #[test]
    fn rate_limited_detection() {
        assert!(is_rate_limited(429, ""));
        assert!(is_rate_limited(200, "RESOURCE_EXHAUSTED: quota"));
        assert!(!is_rate_limited(200, "all good"));
        assert!(!is_rate_limited(500, "internal error"));
    }

    #[test]
    fn thinking_config_per_model() {
        assert_eq!(thinking_config_for("gemini-3.5-flash"), Some(json!({ "thinkingBudget": 0 })));
        assert_eq!(thinking_config_for("gemini-3.1-pro-preview"), Some(json!({ "thinkingBudget": 128 })));
        assert_eq!(thinking_config_for("gemini-1.5-flash"), None);
    }

    #[test]
    fn fmt_timestamp_hms_formats() {
        assert_eq!(fmt_timestamp_hms(0.0), "00:00:00.000");
        assert_eq!(fmt_timestamp_hms(62.5), "00:01:02.500");
        assert_eq!(fmt_timestamp_hms(3661.25), "01:01:01.250");
        assert_eq!(fmt_timestamp_hms(-5.0), "00:00:00.000");
    }

    fn sample_request() -> TranscribeRequest {
        TranscribeRequest {
            run_id: 1,
            path: "clip.mp3".into(),
            api_key: "k".into(),
            api_base: "https://host/v1beta/models/".into(),
            model: "gemini-3.5-flash".into(),
            language_hint: String::new(),
        }
    }

    #[test]
    fn build_body_adds_resume_clause_only_when_resuming() {
        let req = sample_request();
        let first = build_transcribe_body(&req, "audio/mp3", "files/abc", None);
        let first_txt = first["contents"][0]["parts"][0]["text"].as_str().unwrap();
        assert!(!first_txt.contains("Continue transcribing"), "first pass must not resume");

        let cont = build_transcribe_body(&req, "audio/mp3", "files/abc", Some(62.5));
        let cont_txt = cont["contents"][0]["parts"][0]["text"].as_str().unwrap();
        assert!(cont_txt.contains("Continue transcribing ONLY"));
        assert!(cont_txt.contains("00:01:02.500"), "resume timestamp must be embedded");
    }

    #[test]
    fn sort_dedup_clamp_orders_dedups_and_clamps() {
        let mut cues = vec![
            Cue { start: 5.0, end: 6.0, text: "b".into() },
            Cue { start: 1.0, end: 2.0, text: "a".into() },
            Cue { start: 5.0, end: 6.0, text: "b".into() }, // exact duplicate
            Cue { start: 7.0, end: 7.0, text: "c".into() }, // zero-length span
        ];
        sort_dedup_clamp(&mut cues);
        assert_eq!(cues.len(), 3, "exact duplicate dropped");
        assert_eq!(cues[0].text, "a");
        assert_eq!(cues[1].text, "b");
        assert!(cues[2].end > cues[2].start, "zero span clamped forward");
    }
}
