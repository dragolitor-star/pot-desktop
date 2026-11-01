// Task B — Live (real-time) captions from system audio (NEXT-AGENT §2 item 3).
//
// Strictly ADDITIVE: the existing FILE-based transcription in `media.rs`
// (`transcribe_media` / `cancel_transcription`) is left byte-for-byte intact. This
// module captures the system audio MIX via loopback — so it works for ANY desktop
// or browser video, with no site/app integration (it is loopback-based, NOT a
// browser extension) — slices it into short rolling chunks, REUSES the same Gemini
// backend the file flow uses (here via INLINE base64 audio for low latency instead
// of the File-API upload/poll), translates each chunk with a Gemini
// `generateContent` call (mirroring `document::translate`, glossary honored), and
// streams caption lines to the transparent overlay window.
//
// Audio capture uses `cpal`: on Windows, opening the default OUTPUT device as an
// INPUT transparently enables WASAPI loopback (it captures whatever is playing).
// The capture callback runs on cpal's own audio thread, so the session thread's
// blocking network calls never stall capture.
//
// Events (emit_all; every event carries `runId` so a stale session is ignored):
//   live_caption          { runId, partial, final, translated, ts }
//   live_caption://state  { runId, state: "listening"|"transcribing"|"stopped"|"error", message }

use base64::{engine::general_purpose, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Manager;

// Only one live-caption session runs at a time. A newer session (different id) or
// an explicit stop makes the older thread break at the next checkpoint.
static ACTIVE_RUN: AtomicU64 = AtomicU64::new(0);
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

const EVT_CAPTION: &str = "live_caption";
const EVT_STATE: &str = "live_caption://state";

// Gemini downsamples audio to 16 kHz internally, so there is no quality benefit to
// shipping a higher rate — resampling down keeps the inline base64 payload small.
const TARGET_SAMPLE_RATE: u32 = 16_000;
const DEFAULT_CHUNK_SECONDS: f64 = 5.0;
const MIN_CHUNK_SECONDS: f64 = 2.0;
const MAX_CHUNK_SECONDS: f64 = 15.0;
// Bound capture memory if the network stalls: the callback drops the oldest samples
// once the raw buffer exceeds this many seconds of audio.
const MAX_BUFFERED_SECONDS: f64 = 30.0;
// Below this RMS a chunk is treated as silence and skipped (a poor-man's VAD that
// avoids burning API calls on quiet gaps).
const SILENCE_RMS: f32 = 0.004;
const MAX_OUTPUT_TOKENS: u32 = 2048;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryPair {
    pub source_term: String,
    pub target_term: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveCaptionRequest {
    pub run_id: u64,
    pub api_key: String,
    /// API base URL, already resolved by the frontend (ends with `models/`).
    pub api_base: String,
    pub model: String,
    /// Spoken-language hint (e.g. "English"); empty/"Auto" lets the model detect.
    #[serde(default)]
    pub from_label: String,
    /// Target language label (e.g. "Turkish"); empty or == from ⇒ no translation.
    #[serde(default)]
    pub to_label: String,
    /// Seconds of audio per transcription chunk (clamped to [2, 15]).
    #[serde(default)]
    pub chunk_seconds: f64,
    #[serde(default)]
    pub glossary: Vec<GlossaryPair>,
}

/// Start a live-caption session and return immediately. Captions and lifecycle
/// updates are delivered through the events documented above.
#[tauri::command]
pub fn start_live_caption(app_handle: tauri::AppHandle, request: LiveCaptionRequest) {
    let run_id = request.run_id;
    ACTIVE_RUN.store(run_id, Ordering::SeqCst);
    CANCEL_FLAG.store(false, Ordering::SeqCst);
    std::thread::spawn(move || run_session(app_handle, request));
}

/// Stop the currently-running live-caption session (releases the audio device).
#[tauri::command]
pub fn stop_live_caption() {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
}

/// True when this session should stop — explicit stop, or a newer session took over.
fn should_stop(run_id: u64) -> bool {
    CANCEL_FLAG.load(Ordering::SeqCst) || ACTIVE_RUN.load(Ordering::SeqCst) != run_id
}

fn run_session(app: tauri::AppHandle, req: LiveCaptionRequest) {
    let run_id = req.run_id;

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            emit_state(&app, run_id, "error", &format!("HTTP client init failed: {e}"));
            return;
        }
    };

    // Open the loopback capture stream. The callback pushes mono samples (at the
    // device's native rate) into `shared`; we resample per chunk on this thread.
    let shared: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let (stream, src_rate, _channels) = match build_loopback_stream(shared.clone()) {
        Ok(s) => s,
        Err(e) => {
            emit_state(&app, run_id, "error", &e);
            return;
        }
    };
    if let Err(e) = stream.play() {
        emit_state(&app, run_id, "error", &format!("Could not start audio capture: {e}"));
        return;
    }

    let chunk_seconds = clamp_chunk(req.chunk_seconds);
    let chunk_samples_raw = ((src_rate as f64) * chunk_seconds).max(1.0) as usize;
    let glossary_block = build_glossary_block(&req.glossary);
    let url = format!("{}{}:generateContent", req.api_base, req.model);

    emit_state(
        &app,
        run_id,
        "listening",
        "Live caption started — listening to system audio.",
    );

    loop {
        if should_stop(run_id) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));

        // Drain a whole chunk's worth (or more, if a slow network call let it build
        // up) so no audio is lost between iterations.
        let raw = {
            let mut buf = match shared.lock() {
                Ok(b) => b,
                Err(_) => break,
            };
            if buf.len() < chunk_samples_raw {
                continue;
            }
            std::mem::take(&mut *buf)
        };

        let mono16k = resample_to(&raw, src_rate, TARGET_SAMPLE_RATE);
        if rms(&mono16k) < SILENCE_RMS {
            continue; // silence — skip the API call
        }

        emit_state(&app, run_id, "transcribing", "Transcribing…");
        let wav = encode_wav_pcm16(&mono16k, TARGET_SAMPLE_RATE);
        let b64 = general_purpose::STANDARD.encode(&wav);

        let transcript = match transcribe_inline(&client, &url, &req, &b64) {
            Ok(t) => t.trim().to_string(),
            Err(e) => {
                if should_stop(run_id) {
                    break;
                }
                emit_state(&app, run_id, "listening", &format!("Skipped a chunk: {e}"));
                continue;
            }
        };
        if transcript.is_empty() {
            emit_state(&app, run_id, "listening", "");
            continue;
        }

        let ts = now_ms();
        // Show the recognized speech immediately, then fill in the translation.
        emit_caption(&app, run_id, &transcript, "", "", ts);

        let translated = if translation_needed(&req) {
            match translate_text(&client, &url, &req, &glossary_block, &transcript) {
                Ok(t) => t,
                Err(_) => String::new(), // original already shown; leave translation blank
            }
        } else {
            transcript.clone()
        };

        if should_stop(run_id) {
            break;
        }
        emit_caption(&app, run_id, "", &transcript, &translated, ts);
        emit_state(&app, run_id, "listening", "");
    }

    // Dropping the stream releases the audio device — leave it clean on stop.
    drop(stream);
    emit_state(&app, run_id, "stopped", "Live caption stopped.");
}

// ── audio capture (cpal loopback) ─────────────────────────────────────────────

/// Build a WASAPI-loopback (Windows) / default-output capture stream. cpal enables
/// loopback automatically when an output device is opened for input. Returns the
/// stream plus the device's native sample rate and channel count.
fn build_loopback_stream(
    shared: Arc<Mutex<Vec<f32>>>,
) -> Result<(cpal::Stream, u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No default output device available for loopback capture.".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Could not read the output device format: {e}"))?;

    let src_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let max_buffered = ((src_rate as f64) * MAX_BUFFERED_SECONDS) as usize;

    let err_fn = |e| log::warn!("live caption capture error: {e}");
    let shared_cb = shared.clone();

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                push_samples(&shared_cb, downmix_f32(data, channels), max_buffered);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                push_samples(&shared_cb, downmix_int(data, channels, 32768.0), max_buffered);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                let centered: Vec<f32> = data.iter().map(|&s| s as f32 - 32768.0).collect();
                push_samples(&shared_cb, downmix_f32(&centered, channels).iter().map(|v| v / 32768.0).collect(), max_buffered);
            },
            err_fn,
            None,
        ),
        other => {
            return Err(format!(
                "Unsupported audio sample format for loopback capture: {other:?}"
            ))
        }
    }
    .map_err(|e| format!("Could not open the loopback capture stream: {e}"))?;

    Ok((stream, src_rate, channels as u16))
}

fn push_samples(shared: &Arc<Mutex<Vec<f32>>>, mono: Vec<f32>, max_buffered: usize) {
    if let Ok(mut buf) = shared.lock() {
        buf.extend_from_slice(&mono);
        if max_buffered > 0 && buf.len() > max_buffered {
            let drop_n = buf.len() - max_buffered;
            buf.drain(0..drop_n);
        }
    }
}

/// Downmix interleaved f32 frames to mono by averaging channels.
fn downmix_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

/// Downmix interleaved integer frames (i16) to mono f32 in [-1, 1].
fn downmix_int(data: &[i16], channels: usize, scale: f32) -> Vec<f32> {
    if channels <= 1 {
        return data.iter().map(|&s| s as f32 / scale).collect();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().map(|&s| s as f32 / scale).sum::<f32>() / channels as f32)
        .collect()
}

// ── DSP helpers ───────────────────────────────────────────────────────────────

fn clamp_chunk(seconds: f64) -> f64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return DEFAULT_CHUNK_SECONDS;
    }
    seconds.clamp(MIN_CHUNK_SECONDS, MAX_CHUNK_SECONDS)
}

/// Resample mono f32 from `src_rate` to `target_rate` with box-average decimation —
/// a moving average that doubles as an anti-alias low-pass, good enough for ASR. No
/// upsampling: if the source is already at/below the target it is returned as-is.
fn resample_to(samples: &[f32], src_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if src_rate <= target_rate {
        return samples.to_vec();
    }
    let ratio = src_rate as f64 / target_rate as f64;
    let out_len = (samples.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    let mut pos = 0.0f64;
    for _ in 0..out_len {
        let start = pos as usize;
        let end = ((pos + ratio) as usize).min(samples.len());
        let value = if end > start {
            samples[start..end].iter().copied().sum::<f32>() / (end - start) as f32
        } else {
            samples.get(start).copied().unwrap_or(0.0)
        };
        out.push(value);
        pos += ratio;
    }
    out
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Encode mono f32 samples as a 16-bit PCM WAV (RIFF) container.
fn encode_wav_pcm16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * 2; // mono, 16-bit
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

// ── Gemini calls (inline transcribe + translate) ──────────────────────────────

fn translation_needed(req: &LiveCaptionRequest) -> bool {
    let to = req.to_label.trim();
    if to.is_empty() {
        return false;
    }
    // If the spoken language is explicitly the same as the target, skip translation.
    let from = req.from_label.trim();
    if !from.is_empty() && !from.eq_ignore_ascii_case("auto") && from.eq_ignore_ascii_case(to) {
        return false;
    }
    true
}

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

fn safety_settings() -> Value {
    json!([
        { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE" },
        { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE" },
        { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "BLOCK_NONE" },
        { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE" }
    ])
}

fn transcribe_inline(
    client: &reqwest::blocking::Client,
    url: &str,
    req: &LiveCaptionRequest,
    b64_wav: &str,
) -> Result<String, String> {
    let lang_clause = if !req.from_label.is_empty() && !req.from_label.eq_ignore_ascii_case("auto") {
        format!("The spoken language is {}. ", req.from_label)
    } else {
        String::new()
    };
    let instruction = format!(
        "{lang_clause}Transcribe the speech in this short audio clip into plain text in the \
ORIGINAL language. Return ONLY the transcript, with no timestamps, speaker labels, quotation \
marks or notes. If there is no clear speech, return an empty string."
    );

    let mut generation_config = json!({ "maxOutputTokens": MAX_OUTPUT_TOKENS, "temperature": 0 });
    if let Some(tc) = thinking_config_for(&req.model) {
        generation_config["thinkingConfig"] = tc;
    }

    let body = json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": instruction },
                { "inline_data": { "mime_type": "audio/wav", "data": b64_wav } }
            ]
        }],
        "safetySettings": safety_settings(),
        "generationConfig": generation_config
    });

    post_text(client, url, &body, &req.api_key)
}

fn translate_text(
    client: &reqwest::blocking::Client,
    url: &str,
    req: &LiveCaptionRequest,
    glossary_block: &str,
    text: &str,
) -> Result<String, String> {
    let from = if !req.from_label.is_empty() && !req.from_label.eq_ignore_ascii_case("auto") {
        req.from_label.as_str()
    } else {
        "the source language"
    };
    let to = req.to_label.trim();
    let instruction = format!(
        "{glossary_block}You are a professional live-subtitle translator. Translate the text below \
from {from} into {to}. Return ONLY the translation as a single plain-text line — no quotes, no \
transliteration, no notes.\n\nText:\n{text}"
    );

    let mut generation_config = json!({ "maxOutputTokens": MAX_OUTPUT_TOKENS, "temperature": 0.2 });
    if let Some(tc) = thinking_config_for(&req.model) {
        generation_config["thinkingConfig"] = tc;
    }

    let body = json!({
        "contents": [{ "role": "user", "parts": [{ "text": instruction }] }],
        "safetySettings": safety_settings(),
        "generationConfig": generation_config
    });

    let raw = post_text(client, url, &body, &req.api_key)?;
    Ok(strip_wrapping_quotes(raw.trim()).to_string())
}

/// POST a generateContent body and return the first candidate's text part (empty
/// string when the model returns no text — e.g. a silent clip). Errors only on
/// transport/HTTP/parse failures so the session loop can skip a bad chunk and keep
/// listening.
fn post_text(client: &reqwest::blocking::Client, url: &str, body: &Value, api_key: &str) -> Result<String, String> {
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("x-goog-api-key", api_key)
        .json(body)
        .send()
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", truncate(&text, 200)));
    }

    let payload: Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from API: {e}"))?;
    let cand = payload
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .ok_or_else(|| format!("No candidates in response: {}", truncate(&text, 200)))?;

    if let Some(finish) = cand.get("finishReason").and_then(|f| f.as_str()) {
        if finish != "STOP" && finish != "MAX_TOKENS" {
            return Err(format!("Gemini stopped: finishReason={finish}"));
        }
    }

    let raw = cand
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    Ok(raw.to_string())
}

/// Build the glossary instruction block (mirrors `document::translate`: first-seen
/// source term wins, JSON mapping). Empty when there are no usable entries.
fn build_glossary_block(entries: &[GlossaryPair]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut keys: Vec<&str> = Vec::new();
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for e in entries {
        if e.source_term.is_empty() {
            continue;
        }
        if !keys.contains(&e.source_term.as_str()) {
            keys.push(e.source_term.as_str());
            pairs.push((e.source_term.as_str(), e.target_term.as_str()));
        }
    }
    if pairs.is_empty() {
        return String::new();
    }
    let mut mapping = String::from("{\n");
    for (i, (k, v)) in pairs.iter().enumerate() {
        let key = serde_json::to_string(k).unwrap_or_else(|_| "\"\"".into());
        let val = serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
        mapping.push_str("  ");
        mapping.push_str(&key);
        mapping.push_str(": ");
        mapping.push_str(&val);
        if i + 1 < pairs.len() {
            mapping.push(',');
        }
        mapping.push('\n');
    }
    mapping.push('}');
    format!(
        "[Glossary — use these mappings strictly. Mappings are JSON: source_term → \
target_term.]\n{mapping}\n\n"
    )
}

fn strip_wrapping_quotes(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        let first = bytes[0];
        let last = bytes[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].trim();
        }
    }
    trimmed
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── event emit helpers ─────────────────────────────────────────────────────────

fn emit_caption(app: &tauri::AppHandle, run_id: u64, partial: &str, final_: &str, translated: &str, ts: u64) {
    let payload = json!({
            "runId": run_id,
            "partial": partial,
            "final": final_,
            "translated": translated,
            "ts": ts,
        });
    app.trigger_global(EVT_CAPTION, Some(payload.to_string()));
    let _ = app.emit_all(EVT_CAPTION, payload);
}

fn emit_state(app: &tauri::AppHandle, run_id: u64, state: &str, message: &str) {
    let payload = json!({ "runId": run_id, "state": state, "message": message });
    app.trigger_global(EVT_STATE, Some(payload.to_string()));
    let _ = app.emit_all(EVT_STATE, payload);
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(s: &str, t: &str) -> GlossaryPair {
        GlossaryPair { source_term: s.into(), target_term: t.into() }
    }

    fn req(from: &str, to: &str) -> LiveCaptionRequest {
        LiveCaptionRequest {
            run_id: 1,
            api_key: "k".into(),
            api_base: "https://host/v1beta/models/".into(),
            model: "gemini-3.5-flash".into(),
            from_label: from.into(),
            to_label: to.into(),
            chunk_seconds: 0.0,
            glossary: vec![],
        }
    }

    #[test]
    fn clamp_chunk_defaults_and_bounds() {
        assert_eq!(clamp_chunk(0.0), DEFAULT_CHUNK_SECONDS);
        assert_eq!(clamp_chunk(-3.0), DEFAULT_CHUNK_SECONDS);
        assert_eq!(clamp_chunk(f64::NAN), DEFAULT_CHUNK_SECONDS);
        assert_eq!(clamp_chunk(1.0), MIN_CHUNK_SECONDS);
        assert_eq!(clamp_chunk(100.0), MAX_CHUNK_SECONDS);
        assert_eq!(clamp_chunk(7.0), 7.0);
    }

    #[test]
    fn downmix_averages_stereo_to_mono() {
        // interleaved L,R: (1,-1) -> 0, (0.5,0.5) -> 0.5
        let stereo = vec![1.0, -1.0, 0.5, 0.5];
        let mono = downmix_f32(&stereo, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
        // mono passthrough is identity
        assert_eq!(downmix_f32(&[0.1, 0.2, 0.3], 1), vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn resample_halves_count_at_double_rate() {
        // 32 kHz -> 16 kHz: roughly half the samples, box-averaged in pairs.
        let src: Vec<f32> = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let out = resample_to(&src, 32_000, 16_000);
        assert_eq!(out.len(), 3);
        for v in out {
            assert!((v - 0.5).abs() < 1e-6, "pairwise average should be 0.5");
        }
    }

    #[test]
    fn resample_passthrough_when_not_downsampling() {
        let src = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_to(&src, 16_000, 16_000), src);
        assert_eq!(resample_to(&src, 8_000, 16_000), src); // no upsampling
        assert!(resample_to(&[], 48_000, 16_000).is_empty());
    }

    #[test]
    fn rms_silence_vs_signal() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0, 0.0, 0.0]), 0.0);
        assert!((rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < 1e-6);
        assert!(rms(&[0.001, -0.001]) < SILENCE_RMS, "near-silence is below the gate");
    }

    #[test]
    fn wav_header_is_well_formed() {
        let wav = encode_wav_pcm16(&[0.0, 1.0, -1.0], 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // 44-byte header + 3 samples * 2 bytes
        assert_eq!(wav.len(), 44 + 6);
        // data chunk length little-endian = 6
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 6);
        // sample rate echoed at offset 24
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16_000);
        // +1.0 clamps to 32767, -1.0 clamps to -32767
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), 32767);
        assert_eq!(i16::from_le_bytes([wav[48], wav[49]]), -32767);
    }

    #[test]
    fn translation_needed_rules() {
        assert!(translation_needed(&req("Auto", "Turkish")));
        assert!(translation_needed(&req("English", "Turkish")));
        assert!(!translation_needed(&req("English", ""))); // no target
        assert!(!translation_needed(&req("Turkish", "turkish"))); // same lang, case-insensitive
        assert!(translation_needed(&req("", "Turkish")));
    }

    #[test]
    fn glossary_block_dedups_first_seen_and_escapes() {
        let entries = vec![pair("API", "API"), pair("API", "ignored"), pair("he said \"hi\"", "dedi")];
        let block = build_glossary_block(&entries);
        assert!(block.contains("\"API\": \"API\""));
        assert!(!block.contains("ignored"), "first-seen mapping must win");
        assert!(block.contains("he said \\\"hi\\\""));
        assert_eq!(build_glossary_block(&[]), "");
    }

    #[test]
    fn strip_quotes_unwraps_one_layer() {
        assert_eq!(strip_wrapping_quotes("\"merhaba\""), "merhaba");
        assert_eq!(strip_wrapping_quotes("'merhaba'"), "merhaba");
        assert_eq!(strip_wrapping_quotes("merhaba"), "merhaba");
        assert_eq!(strip_wrapping_quotes("\"a\" and \"b\""), "a\" and \"b");
    }
}
