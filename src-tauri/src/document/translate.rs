// Background document-translation loop (Phase 2 §7 / HANDOFF §7).
//
// The renderer used to run the whole batch/RPM/retry/parse loop on the JS main
// thread (`handleStartTranslation`). For very long PDFs that loop — with its RPM
// sleeps, network waits and JSON parsing — competed with the UI thread and could
// make the window feel stuck. This module moves that loop into a dedicated Rust
// background thread and reports progress back to the `document` window via Tauri
// events, so the UI stays responsive and cancellation is instant.
//
// Scope: the Gemini structured-output path only (the project is Gemini-first and
// `responseSchema` is the reliable path — HANDOFF §8 #1). Non-Gemini engines keep
// the in-renderer numbered-marker loop. Extraction, paragraph splitting, dedup,
// code/formula pass-through and the translation cache stay frontend/SQLite-owned
// (single-sourced) — this module receives only the unique, cache-missed source
// paragraphs and returns their translations, batched + paced here.
//
// Events emitted to the frontend (all carry `runId` so stale runs are ignored):
//   document://progress  { runId, batchIndex, batchCount, doneUnique, totalUnique, text }
//   document://batch     { runId, results: [{ source, translation|null, error|null }] }
//   document://done      { runId, status: "completed"|"rate_limited"|"cancelled"|"error",
//                          message }

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::Manager;

// Only one document translation runs at a time. A newer run (different id) or an
// explicit cancel makes the older thread stop at the next batch boundary.
static ACTIVE_RUN: AtomicU64 = AtomicU64::new(0);
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

const EVT_PROGRESS: &str = "document://progress";
const EVT_BATCH: &str = "document://batch";
const EVT_DONE: &str = "document://done";

const MAX_PARAS_PER_BATCH: usize = 25;
const MIN_CHAR_BUDGET: usize = 1000;
const MAX_OUTPUT_TOKENS: u32 = 32768;
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryPair {
    pub source_term: String,
    pub target_term: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocTranslateRequest {
    pub run_id: u64,
    /// Unique, cache-missed, pass-through-filtered source paragraphs (in order).
    pub texts: Vec<String>,
    pub api_key: String,
    /// API base URL, already resolved by the frontend and ending with '/'.
    pub api_base: String,
    pub model: String,
    /// Gemini language labels (e.g. "English"/"Auto", "Turkish").
    pub from_label: String,
    pub to_label: String,
    pub chars_per_request: usize,
    pub rpm_limit: u32,
    #[serde(default)]
    pub glossary: Vec<GlossaryPair>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchResult {
    source: String,
    translation: Option<String>,
    error: Option<String>,
}

/// Kick off the background translation loop and return immediately. Progress and
/// results are delivered through the events documented above.
#[tauri::command]
pub fn translate_document(app_handle: tauri::AppHandle, request: DocTranslateRequest) {
    let run_id = request.run_id;
    // Claim the active slot and clear any stale cancel from a previous run.
    ACTIVE_RUN.store(run_id, Ordering::SeqCst);
    CANCEL_FLAG.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        run_translation(app_handle, request);
    });
}

/// Request cancellation of the currently-running document translation.
#[tauri::command]
pub fn cancel_document_translation() {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
}

/// True when this run should stop — explicit cancel, or a newer run superseded it.
fn should_stop(run_id: u64) -> bool {
    CANCEL_FLAG.load(Ordering::SeqCst) || ACTIVE_RUN.load(Ordering::SeqCst) != run_id
}

fn run_translation(app: tauri::AppHandle, req: DocTranslateRequest) {
    let run_id = req.run_id;
    let total_unique = req.texts.len();
    let batches = batch_by_budget(&req.texts, req.chars_per_request);
    let batch_count = batches.len();

    let glossary_block = build_glossary_block(&req.glossary);
    let url = format!("{}{}:generateContent", req.api_base, req.model);
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            emit_done(
                &app,
                run_id,
                "error",
                &format!("HTTP client init failed: {e}"),
            );
            return;
        }
    };

    let mut limiter = RpmLimiter::new(req.rpm_limit);
    let mut done_unique = 0usize;

    for (b_idx, batch) in batches.iter().enumerate() {
        if should_stop(run_id) {
            emit_done(&app, run_id, "cancelled", "Translation cancelled.");
            return;
        }

        emit_progress(
            &app,
            run_id,
            b_idx + 1,
            batch_count,
            done_unique,
            total_unique,
            &format!(
                "Batch {}/{} · structured JSON · {}/{} unique paragraphs",
                b_idx + 1,
                batch_count,
                done_unique,
                total_unique
            ),
        );

        if should_stop(run_id) {
            emit_done(&app, run_id, "cancelled", "Translation cancelled.");
            return;
        }

        match translate_batch_with_retry(
            &client,
            &url,
            &req,
            &glossary_block,
            batch,
            run_id,
            &mut limiter,
        ) {
            Ok(mut translations) => {
                for (source, translation) in batch.iter().zip(&mut translations) {
                    if translation.as_deref().is_some_and(|text| {
                        residual_cjk(text, &req.to_label) || protected_token_loss(source, text)
                    }) {
                        *translation = None;
                    }
                }
                // Validate before emitting anything that the renderer can cache.
                // One isolated repair per bad item cannot shift adjacent IDs.
                for (i, source) in batch.iter().enumerate() {
                    if translations[i].as_deref().is_some_and(|t| {
                        !residual_cjk(t, &req.to_label) && !protected_token_loss(source, t)
                    }) {
                        continue;
                    }
                    translations[i] = None;
                    if should_stop(run_id) {
                        break;
                    }
                    let body = build_request_body(&req, &glossary_block, &[source.clone()]);
                    limiter.acquire(|| should_stop(run_id));
                    if should_stop(run_id) {
                        break;
                    }
                    match translate_batch_once(&client, &url, &body, 1, &req.api_key) {
                        Ok(retry) => {
                            translations[i] = retry.into_iter().next().flatten().filter(|t| {
                                !residual_cjk(t, &req.to_label) && !protected_token_loss(source, t)
                            });
                        }
                        Err(e) if e.rate_limited => {
                            // Publish valid results before stopping, so they survive a quota reset.
                            let results: Vec<_> = batch
                                .iter()
                                .zip(&translations)
                                .map(|(s, t)| BatchResult {
                                    source: s.clone(),
                                    translation: t.clone(),
                                    error: t
                                        .is_none()
                                        .then(|| "Translation validation failed".into()),
                                })
                                .collect();
                            emit_batch(&app, run_id, &results);
                            emit_done(&app, run_id, "rate_limited", &e.message);
                            return;
                        }
                        Err(e) => {
                            log::warn!("Single-item translation repair failed: {}", e.message)
                        }
                    }
                }
                let results: Vec<BatchResult> = batch
                    .iter()
                    .enumerate()
                    .map(|(i, source)| match translations.get(i).and_then(|t| t.clone()) {
                        Some(tr) => BatchResult {
                            source: source.clone(),
                            translation: Some(tr),
                            error: None,
                        },
                        None => BatchResult {
                            source: source.clone(),
                            translation: None,
                            error: Some("Missing, ambiguous, or untranslated item after single-item retry.".into()),
                        },
                    })
                    .collect();
                emit_batch(&app, run_id, &results);
            }
            Err(err) => {
                // A 429 (rate limit / daily quota) means hammering the rest just burns
                // more quota and piles up identical failures — stop now. Paragraphs done
                // so far are already cached by the frontend, so a re-run continues from
                // where it left off once the limit resets.
                if err.rate_limited {
                    emit_done(&app, run_id, "rate_limited", &err.message);
                    return;
                }
                let results: Vec<BatchResult> = batch
                    .iter()
                    .map(|source| BatchResult {
                        source: source.clone(),
                        translation: None,
                        error: Some(err.message.clone()),
                    })
                    .collect();
                emit_batch(&app, run_id, &results);
            }
        }

        done_unique += batch.len();
    }

    if should_stop(run_id) {
        emit_done(&app, run_id, "cancelled", "Translation cancelled.");
    } else {
        emit_done(&app, run_id, "completed", "Document translation completed.");
    }
}

// ── batching ────────────────────────────────────────────────────────────────

/// Pack source paragraphs into batches bounded by a source-character budget and a
/// hard per-batch paragraph cap (mirrors the previous renderer logic). Each batch
/// is filled as full as is safe so the document costs the fewest possible calls.
fn batch_by_budget(texts: &[String], chars_per_request: usize) -> Vec<Vec<String>> {
    let budget = chars_per_request.max(MIN_CHAR_BUDGET);
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_chars = 0usize;
    for text in texts {
        if !cur.is_empty() && (cur_chars + text.len() > budget || cur.len() >= MAX_PARAS_PER_BATCH)
        {
            batches.push(std::mem::take(&mut cur));
            cur_chars = 0;
        }
        cur_chars += text.len();
        cur.push(text.clone());
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

// ── glossary ──────────────────────────────────────────────────────────────--

/// Build the glossary instruction block, mirroring the frontend
/// `applyGlossaryToPrompt` (first-seen source_term wins, JSON mapping). Returns an
/// empty string when there are no usable entries.
fn build_glossary_block(entries: &[GlossaryPair]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    // Preserve first-seen order so the highest-priority mapping sticks.
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
        "[Glossary — use these mappings strictly. Do not deviate. Apply exactly as \
written, preserving case where the source matches case-sensitively. Mappings are \
JSON: source_term → target_term.]\n{mapping}\n\n"
    )
}

// ── Gemini structured-output call ─────────────────────────────────────────────

struct BatchError {
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

pub(crate) fn thinking_config_for(model: &str) -> Option<Value> {
    let m = model.to_lowercase();
    let is_thinking = m.contains("gemini-2.5") || m.contains("gemini-3");
    if !is_thinking {
        return None;
    }
    if m.contains("gemini-3") {
        return Some(
            json!({"thinkingLevel":if m.contains("pro") || m.contains("3.8") || m.contains("3.7") {"low"}else{"minimal"}}),
        );
    }
    // Flash/Flash-Lite can fully disable thinking (budget 0); Pro floor is 128.
    if m.contains("pro") {
        Some(json!({ "thinkingBudget": 128 }))
    } else {
        Some(json!({ "thinkingBudget": 0 }))
    }
}

fn build_request_body(req: &DocTranslateRequest, glossary_block: &str, batch: &[String]) -> Value {
    let from_label = if !req.from_label.is_empty() && req.from_label != "Auto" {
        req.from_label.as_str()
    } else {
        "the source language"
    };
    let to = &req.to_label;

    let instruction = format!(
        "{glossary_block}You are a professional translator. Translate the \"text\" \
field of every item in the JSON array below from {from_label} into {to}. Return one \
result per item, keeping the same \"id\". Translate every item — never copy the \
source untranslated and never omit, merge, reorder, or add items. Preserve numbers, \
inline punctuation and meaning; output natural, fluent {to}. Do not add notes or \
explanations. Never echo CJK for a non-CJK target. Preserve technical tokens \
(clb/fld, prm.txt, laser_data.bin, sts code, helm set 0 1018, ∆a, ∆b, x, y, theta) \
exactly in their grammatical place. Never duplicate sentences or invent extra text."
    );

    let items: Vec<Value> = batch
        .iter()
        .enumerate()
        .map(|(i, t)| json!({ "id": i + 1, "text": t }))
        .collect();
    let user_text = serde_json::to_string(&items).unwrap_or_default();

    let mut generation_config = json!({
        "maxOutputTokens": MAX_OUTPUT_TOKENS,
        "responseMimeType": "application/json",
        "responseSchema": {
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "id": { "type": "INTEGER" },
                    "translation": { "type": "STRING" }
                },
                "required": ["id", "translation"],
                "propertyOrdering": ["id", "translation"]
            }
        }
    });
    if let Some(tc) = thinking_config_for(&req.model) {
        generation_config["thinkingConfig"] = tc;
    }

    json!({
        "systemInstruction": { "parts": [{ "text": instruction }] },
        "contents": [{ "role": "user", "parts": [{ "text": user_text }] }],
        "safetySettings": [
            { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE" }
        ],
        "generationConfig": generation_config
    })
}

fn translate_batch_with_retry(
    client: &reqwest::blocking::Client,
    url: &str,
    req: &DocTranslateRequest,
    glossary_block: &str,
    batch: &[String],
    run_id: u64,
    limiter: &mut RpmLimiter,
) -> Result<Vec<Option<String>>, BatchError> {
    let body = build_request_body(req, glossary_block, batch);
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(1000);
    loop {
        if should_stop(run_id) {
            return Err(BatchError {
                message: "cancelled".into(),
                rate_limited: false,
            });
        }
        limiter.acquire(|| should_stop(run_id));
        if should_stop(run_id) {
            return Err(BatchError {
                message: "cancelled".into(),
                rate_limited: false,
            });
        }
        match translate_batch_once(client, url, &body, batch.len(), &req.api_key) {
            Ok(v) => return Ok(v),
            Err(err) => {
                // 429 / quota errors are never retried — bubble so the loop aborts.
                if err.rate_limited {
                    return Err(err);
                }
                attempt += 1;
                if attempt > MAX_RETRIES || should_stop(run_id) {
                    return Err(err);
                }
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
}

fn translate_batch_once(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &Value,
    expected: usize,
    api_key: &str,
) -> Result<Vec<Option<String>>, BatchError> {
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("x-goog-api-key", api_key)
        .json(body)
        .send()
        .map_err(|e| BatchError {
            message: format!("Network error: {e}"),
            rate_limited: false,
        })?;

    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();

    if status < 200 || status >= 300 {
        return Err(BatchError {
            message: format!("Http Request Error\nHttp Status: {status}\n{text}"),
            rate_limited: is_rate_limited(status, &text),
        });
    }

    let payload: Value = serde_json::from_str(&text).map_err(|e| BatchError {
        message: format!("Invalid JSON from API: {e}"),
        rate_limited: false,
    })?;

    let candidates = payload.get("candidates").and_then(|c| c.as_array());
    let cand = match candidates.and_then(|c| c.first()) {
        Some(c) => c,
        None => {
            return Err(BatchError {
                message: format!("No candidates in response: {text}"),
                rate_limited: is_rate_limited(status, &text),
            })
        }
    };

    if let Some(finish) = cand.get("finishReason").and_then(|f| f.as_str()) {
        if finish != "STOP" && finish != "MAX_TOKENS" {
            return Err(BatchError {
                message: format!("Gemini stopped: finishReason={finish}"),
                rate_limited: false,
            });
        }
    }

    let raw = cand
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| BatchError {
            message: format!("No text part in candidate: {text}"),
            rate_limited: false,
        })?;

    Ok(map_structured_array(raw, expected))
}

/// Parse the model's `[{ id, translation }]` JSON into a slot-aligned vector of
/// length `expected` (None for any id the model dropped). Defensive against a
/// MAX_TOKENS-truncated tail (mirrors the frontend `parseStructuredArray`).
fn map_structured_array(raw: &str, expected: usize) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![None; expected];
    let mut seen = vec![false; expected];
    let mut duplicates = vec![false; expected];
    for el in parse_structured_array(raw) {
        let id = el.get("id").and_then(|v| v.as_i64());
        let translation = el.get("translation").and_then(|v| v.as_str());
        if let (Some(id), Some(tr)) = (id, translation) {
            if id >= 1 && (id as usize) <= expected {
                let i = (id - 1) as usize;
                if seen[i] {
                    duplicates[i] = true;
                }
                seen[i] = true;
                let trimmed = tr.trim();
                if !trimmed.is_empty() {
                    out[(id - 1) as usize] = Some(trimmed.to_string());
                }
            }
        }
    }
    for (i, duplicate) in duplicates.iter().enumerate() {
        if *duplicate {
            out[i] = None;
        }
    }
    out
}

/// Preserve command identifiers and filenames even when an otherwise valid
/// structured response pairs an item with a neighboring paragraph. Substring
/// matching tolerates PDF-fragmented identifiers such as `alg_b ias_left`.
pub(crate) fn protected_token_loss(source: &str, target: &str) -> bool {
    static TOKENS: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"[A-Za-z][A-Za-z0-9_.-]*\.(?:txt|bin|json|xml|ini|cfg|log|csv|dll|exe)|[A-Za-z][A-Za-z0-9_]*_[A-Za-z0-9_]+").unwrap()
    });
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for token in TOKENS.find_iter(source) {
        *counts.entry(token.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .any(|(token, count)| target.matches(token).count() < count)
}

pub(crate) fn residual_cjk(text: &str, target: &str) -> bool {
    let target = target.to_lowercase();
    let code = target.split(['_', '-']).next().unwrap_or("");
    let cjk_target = matches!(code, "zh" | "ja" | "ko")
        || ["chinese", "japanese", "korean", "中文", "日本", "한국"]
            .iter()
            .any(|p| target.starts_with(p));
    !cjk_target && super::pdf::contains_han(text)
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
    // Last resort: salvage every complete top-level {…} object so a truncated
    // batch still yields its finished items.
    salvage_objects(raw)
}

/// String-aware brace matcher — collects each balanced top-level `{…}` object and
/// parses it, skipping fragments that don't parse. Brace characters inside JSON
/// strings are ignored so a translation containing `{`/`}` doesn't split objects.
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

// ── RPM limiter ───────────────────────────────────────────────────────────--

/// Sliding-window requests-per-minute limiter. `acquire` blocks (in short slices,
/// so cancellation stays responsive) until a request slot is free.
struct RpmLimiter {
    rpm: usize,
    window: Duration,
    stamps: VecDeque<Instant>,
}

impl RpmLimiter {
    fn new(rpm: u32) -> Self {
        RpmLimiter {
            rpm: (rpm as usize).max(1),
            window: Duration::from_secs(60),
            stamps: VecDeque::new(),
        }
    }

    fn acquire<F: Fn() -> bool>(&mut self, should_abort: F) {
        loop {
            let now = Instant::now();
            while let Some(&front) = self.stamps.front() {
                if now.duration_since(front) >= self.window {
                    self.stamps.pop_front();
                } else {
                    break;
                }
            }
            if self.stamps.len() < self.rpm {
                self.stamps.push_back(now);
                return;
            }
            if should_abort() {
                return;
            }
            // Sleep in small slices so a cancel doesn't wait out a full window.
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

// ── event emit helpers ────────────────────────────────────────────────────--

fn emit_progress(
    app: &tauri::AppHandle,
    run_id: u64,
    batch_index: usize,
    batch_count: usize,
    done_unique: usize,
    total_unique: usize,
    text: &str,
) {
    let payload = json!({
            "runId": run_id,
            "batchIndex": batch_index,
            "batchCount": batch_count,
            "doneUnique": done_unique,
            "totalUnique": total_unique,
            "text": text,
        });
    app.trigger_global(EVT_PROGRESS, Some(payload.to_string()));
    let _ = app.emit_all(EVT_PROGRESS, payload);
}

fn emit_batch(app: &tauri::AppHandle, run_id: u64, results: &[BatchResult]) {
    let payload = json!({
            "runId": run_id,
            "results": results,
        });
    app.trigger_global(EVT_BATCH, Some(payload.to_string()));
    let _ = app.emit_all(EVT_BATCH, payload);
}

fn emit_done(app: &tauri::AppHandle, run_id: u64, status: &str, message: &str) {
    let payload = json!({
            "runId": run_id,
            "status": status,
            "message": message,
        });
    app.trigger_global(EVT_DONE, Some(payload.to_string()));
    let _ = app.emit_all(EVT_DONE, payload);
}

// ── tests ──────────────────────────────────────────────────────────────────-

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighboring_commands_and_missing_filenames_are_rejected() {
        assert!(protected_token_loss(
            "查看 castor_fork_calib_set read_o3x",
            "castor_fork_calib_set write_o3x"
        ));
        assert!(protected_token_loss(
            "下载 rcu_hw_cfg_out.json",
            "Dosyayı indirin"
        ));
        assert!(protected_token_loss("prm.txt prm.txt", "prm.txt"));
    }

    #[test]
    fn pdf_split_identifiers_and_localized_units_keep_technical_values() {
        assert!(!protected_token_loss(
            "有 alg_b ias_left 和 delt_x 1",
            "alg_bias_left ve delt_x1"
        ));
        assert!(!protected_token_loss("0.3m/s 的速度", "0,3 m/s hız"));
        assert!(!protected_token_loss(
            "下载 rcu_hw_cfg_out.json",
            "rcu_hw_cfg_out.json dosyasını indirin"
        ));
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn batching_respects_char_budget() {
        let texts = vec![s("aaaa"), s("bbbb"), s("cccc")];
        // budget is clamped to MIN_CHAR_BUDGET (1000), so all fit in one batch.
        let batches = batch_by_budget(&texts, 1);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 3);
    }

    #[test]
    fn batching_splits_on_paragraph_cap() {
        let texts: Vec<String> = (0..60).map(|i| format!("p{i}")).collect();
        let batches = batch_by_budget(&texts, 100_000);
        assert_eq!(batches.len(), 3); // 25 + 25 + 10
        assert_eq!(batches[0].len(), MAX_PARAS_PER_BATCH);
        assert_eq!(batches[2].len(), 10);
    }

    #[test]
    fn batching_keeps_order_and_all_items() {
        let texts: Vec<String> = (0..7).map(|i| "x".repeat(400) + &i.to_string()).collect();
        let batches = batch_by_budget(&texts, 1000);
        let flat: Vec<String> = batches.into_iter().flatten().collect();
        assert_eq!(flat, texts);
    }

    #[test]
    fn batching_empty_input_yields_no_batches() {
        let texts: Vec<String> = Vec::new();
        let batches = batch_by_budget(&texts, 1000);
        assert!(batches.is_empty());
    }

    #[test]
    fn batching_oversized_paragraph_gets_its_own_batch() {
        // A paragraph larger than the whole budget must not be dropped or split —
        // it forms its own batch, and an oversized paragraph between two others
        // flushes each neighbour into a separate batch too.
        let small = "a".repeat(900);
        let huge = "y".repeat(5000);
        let texts = vec![small.clone(), huge.clone(), small.clone()];
        let batches = batch_by_budget(&texts, 1000);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[1].len(), 1);
        assert_eq!(batches[1][0], huge); // content preserved verbatim
    }

    #[test]
    fn batching_groups_multiple_paragraphs_per_batch_under_budget() {
        // Five 450-char paragraphs under a 1000-char budget pack two per batch
        // until the budget is hit: [2, 2, 1].
        let texts: Vec<String> = (0..5).map(|_| "z".repeat(450)).collect();
        let batches = batch_by_budget(&texts, 1000);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[1].len(), 2);
        assert_eq!(batches[2].len(), 1);
    }

    #[test]
    fn batching_exact_budget_boundary_keeps_paragraphs_together() {
        // Two 500-char paragraphs sum to exactly the 1000-char budget; the split
        // condition is strictly greater-than, so they stay in one batch.
        let texts = vec!["m".repeat(500), "n".repeat(500)];
        let batches = batch_by_budget(&texts, 1000);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
    }

    #[test]
    fn glossary_block_empty_when_no_entries() {
        assert_eq!(build_glossary_block(&[]), "");
    }

    #[test]
    fn glossary_block_dedups_first_seen_and_escapes() {
        let entries = vec![
            GlossaryPair {
                source_term: s("API"),
                target_term: s("API"),
            },
            GlossaryPair {
                source_term: s("API"),
                target_term: s("ignored"),
            },
            GlossaryPair {
                source_term: s("he said \"hi\""),
                target_term: s("dedi"),
            },
        ];
        let block = build_glossary_block(&entries);
        assert!(block.contains("\"API\": \"API\""));
        assert!(!block.contains("ignored"), "first-seen mapping must win");
        // The quote inside the source term must be JSON-escaped.
        assert!(block.contains("he said \\\"hi\\\""));
    }

    #[test]
    fn parse_clean_array() {
        let raw = r#"[{"id":1,"translation":"merhaba"},{"id":2,"translation":"dünya"}]"#;
        let out = map_structured_array(raw, 2);
        assert_eq!(out[0].as_deref(), Some("merhaba"));
        assert_eq!(out[1].as_deref(), Some("dünya"));
    }

    #[test]
    fn parse_truncated_tail_salvages_done_items() {
        // A MAX_TOKENS clip: trailing `]` and last item are missing.
        let raw = r#"[{"id":1,"translation":"bir"},{"id":2,"translation":"iki"},{"id":3,"#;
        let out = map_structured_array(raw, 3);
        assert_eq!(out[0].as_deref(), Some("bir"));
        assert_eq!(out[1].as_deref(), Some("iki"));
        assert_eq!(out[2], None, "dropped/truncated item stays None");
    }

    #[test]
    fn parse_ignores_out_of_range_ids() {
        let raw = r#"[{"id":5,"translation":"x"},{"id":1,"translation":"ok"}]"#;
        let out = map_structured_array(raw, 2);
        assert_eq!(out[0].as_deref(), Some("ok"));
        assert_eq!(out[1], None);
    }

    #[test]
    fn salvage_handles_braces_inside_strings() {
        let raw = r#"[{"id":1,"translation":"f(x) = {a}"}]"#;
        let out = map_structured_array(raw, 1);
        assert_eq!(out[0].as_deref(), Some("f(x) = {a}"));
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
        assert_eq!(
            thinking_config_for("gemini-3.5-flash"),
            Some(json!({ "thinkingLevel": "minimal" }))
        );
        assert_eq!(
            thinking_config_for("gemini-3.1-pro-preview"),
            Some(json!({ "thinkingLevel": "low" }))
        );
        assert_eq!(thinking_config_for("gemini-1.5-flash"), None);
    }
}
