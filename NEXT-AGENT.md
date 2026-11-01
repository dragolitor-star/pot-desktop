# NEXT-AGENT — start here, do NOT re-explore

> **1 Kasım 2026 güncel devir:** Önce [DEVIR-WEB-2026-11-01.md](DEVIR-WEB-2026-11-01.md) dosyasını oku. Tarayıcı workspace'i, Tauri komut köprüsü, son test sonuçları ve tamamlanmamış işler oradadır. Aşağıdaki metin geçmiş oturumların arşividir; eski klasör, tarih, commit yetkisi ve aşamada durma ifadeleri güncel kullanıcı talimatlarının yerine geçmez.

> Read this file only (there is NO `HANDOFF.md` in the repo — this file is the complete
> handoff). Everything you need to start the next task (exact files, line anchors, commands,
> decisions) is below. Do not search the repo to "get oriented" — the orientation is here.
> Last updated: 2026-06-18 (UTC+3) — session bug-fixes for Windows Subtitle window, PDFium version,
> PDF image scan, and layout-export progress (see §1 bullet "BUG FIXES 2026-06-18"). Prior session
> re-verification (2026-05-30) had PAUSED at Phase-5 boundary.

## 0. Project rules (binding — from `.cursorrules` + `HANDOFF.md §2/§11`)
- **No chatter.** No greetings, no chain-of-thought, no summaries. Only the result.
- **Never explain your work unless explicitly asked. Provide only the final result.**
- Code output = only changed/added lines, 1-line technical note max.
- **Never commit / push unless the user explicitly says so.** (User is holding all commits.)
- **Consult before any large architectural change** and **PAUSE at phase boundaries.**
- GUI-dependent results must be verified BY THE USER on Windows (you can't see the window).
- User writes mixed Turkish/English; match the language they open with. Keep paths/identifiers in English.

## 1. Repo state (ground truth — don't re-check unless acting on it)
- **Dir:** `C:\Users\enesk\pot-desktop`  · **Branch:** `master`  · **HEAD:** `8492fab`
  (`feat(pdf): Gemini responseSchema structured output for document translation`).
- **User sign-off (2026-05-30 ~04:08, UTC+3):** user wrote "I approve all changes made
  so far" and explicitly chose **"start Phase 4 now"** when asked at the phase boundary.
  That authorized starting Phase 4 (below). It does NOT mean GUI/network runs happened —
  the ⚠ VERIFY notes still need the user to actually run the app on Windows. **Still NO
  commits** (the user approved the code but never said "commit"; directive: never commit
  unless explicitly told).
- **Uncommitted work (done, NOT committed — user is holding commits):**
  - **Native PDF output assembly (HANDOFF §7 / §8 #3) — DONE + user-verified.**
    Files: `src-tauri/src/document/export.rs` (NEW), `…/document/pdf.rs`, `…/document/mod.rs`,
    `…/cache.rs`, `…/glossary.rs`, `…/main.rs`, `…/window.rs`, `Cargo.toml`/`Cargo.lock`,
    `src-tauri/assets/fonts/DejaVuSans.ttf` (NEW), `.gitignore`.
    New crates: `printpdf 0.9` (`default-features=false`, features `png`+`text_layout`),
    `ttf-parser 0.25`. Key fix landed: per-line absolute placement via
    `Op::SetTextMatrix(TextMatrix::Translate)` in `export.rs::write_lines` (relative `Td`
    was pushing every line after the first off-page).
  - **Code/formula pass-through heuristic (HANDOFF §7) — DONE + tested.**
    `src/utils/passthrough.js` (NEW, exports `shouldPassThrough` + `FORMULA_SIGNAL`),
    imported in `src/window/Document/index.jsx:19`, wired at **two** integration points:
    `index.jsx:434` (batched/page mode dedup loop) and `index.jsx:606` (per-paragraph mode).
    Regression test: `src/utils/passthrough.test.js` — **full suite 14/14 green** (`pnpm test`).
  - **§9 #2 glossary `scope` over-filter (was TASK A below) — DONE + tested.**
    Root cause was the WHERE clause `AND (?3 IS NULL OR scope IS NULL OR scope = ?3)` in
    `src-tauri/src/glossary.rs::query_active_glossary`, which EXCLUDED non-matching scopes
    (e.g. 'tech' rows dropped when scope='document'). Fix: removed that WHERE line entirely
    (scope no longer filters) and made the `ORDER BY` a 3-tier boost
    (matching-scope/no-filter = 0, global NULL = 1, other scope = 2). Directive #4 now means
    BOOST, not exclude. `index.jsx:273` still passes `'document'` and is correct as-is.
    Updated: top-of-file resolution-rules comment, `docs/glossary.md` (rule #2 + the SQL
    snippet). New Rust test `resolution_scope_boosts_not_excludes` proves a 'tech' row is
    still returned but ranked last. **`cargo test glossary` = 23/23 green.**
  - **§7 Rust background translation loop + Tauri progress events (was TASK B) — DONE in
    code + unit-tested; NEEDS USER GUI/NETWORK VERIFICATION on Windows.** (User approved the
    architectural change 2026-05-30 03:05.)
    - NEW `src-tauri/src/document/translate.rs`: command `translate_document` spawns a
      std::thread that batches (char budget + 25-para cap), paces (sliding-window RpmLimiter),
      retries (3× backoff, 429 NOT retried), calls Gemini `generateContent` with the SAME
      `responseSchema` `[{id,translation}]` body the JS path used (reqwest blocking), parses
      defensively (clean array → `[..]` slice → string-aware brace salvage for MAX_TOKENS
      truncation), and emits `document://progress|batch|done` via `emit_all` (every event
      carries `runId`). Cancel via `cancel_document_translation` (static `CANCEL_FLAG` +
      `ACTIVE_RUN`; newer run supersedes older). Glossary instruction rebuilt in Rust to
      mirror `applyGlossaryToPrompt` (first-seen wins, JSON mapping). **No cache/DB access in
      Rust** — cache + dedup + pass-through stay frontend/SQLite-owned (single-sourced).
    - Wired: `document/mod.rs` (`pub mod translate;`), `main.rs` (`use document::translate::*;`
      + both commands in `invoke_handler`).
    - Frontend `src/window/Document/index.jsx`: in the batched (`page`) mode, when engine is
      `geminipro` (`supportsStructured`) it now registers `listen()` handlers, `invoke(
      'translate_document', { request })`, and awaits a `document://done` promise — instead of
      the old in-renderer structured loop (which was deleted along with
      `translateBatchStructuredWithRetry`). Non-Gemini engines keep the in-renderer numbered-
      marker loop (moved into the `else` branch). `handleCancel` also invokes the Rust cancel.
    - geminipro service: `resolveModelAndBase` is now `export`ed so the frontend hands Rust the
      resolved `{ model, base }`.
    - Tests: `cargo test document::translate` = **15/15 green** (batching, glossary block,
      structured parse incl. truncation + braces-in-strings, 429 detection, thinking config).
      `pnpm test` still **14/14**.
    - ⚠ VERIFY (user, Windows, real Gemini key): run a small PDF in Batched mode with Gemini —
      progress streams, translations land, Cancel stops instantly, 429 shows the resume toast,
      glossary terms honored. Agent can't see the GUI or make live API calls.
  - **§7 paragraph-grouping unit tests (was TASK D remainder) — DONE + tested.** Added 4
    edge-case tests for `batch_by_budget` in `src-tauri/src/document/translate.rs` (test mod):
    `batching_empty_input_yields_no_batches`, `batching_oversized_paragraph_gets_its_own_batch`
    (oversize para is never split/dropped and flushes neighbours), `batching_groups_multiple_
    paragraphs_per_batch_under_budget` (450-char paras pack [2,2,1] under a 1000 budget),
    `batching_exact_budget_boundary_keeps_paragraphs_together` (split is strictly `>` budget).
    `cargo test document::translate` = **15/15 green** (was 11/11). No source logic changed —
    tests only; the 2 `cargo` warnings are pre-existing (`window.rs` unused imports), not mine.
  - **Phase 3 — EPUB translation (HANDOFF §1 table row 3) — DONE in code + unit-tested;
    NEEDS USER GUI/NETWORK VERIFICATION on Windows.** (User approved starting Phase 3
    2026-05-30 ~03:25 by choosing "start Phase 3 now".) Mirrors the PDF flow so the whole
    renderer pipeline (dedup → cache → pass-through → batched Gemini `translate_document`)
    and the bilingual viewer are reused unchanged.
    - NEW crate: `quick-xml 0.40.1` (`cargo add`; tiny XML pull-parser, well under the 10 MB
      rule). `zip` (already a dep) does the read+write; no other new crates.
    - NEW `src-tauri/src/document/epub.rs` (+ `pub mod epub;` in `document/mod.rs`,
      `use document::epub::*;` and both commands in `main.rs` invoke_handler):
      - `extract_epub(path) -> Vec<String>`: zip → `META-INF/container.xml` → OPF → spine
        (ordered by `<itemref>`, hrefs resolved/percent-decoded against the OPF dir). Returns
        ONE normalized string per spine chapter (the EPUB analogue of `extract_pdf_pages`'
        per-page strings). Segmentation is **per XML text node**; adjacent text + entity-ref
        events (quick-xml 0.40 emits `&amp;` as a separate `GeneralRef` event) are MERGED into
        one run, runs split at element boundaries (so inline `<b>/<i>/<a>` keep a paragraph
        together but separate blocks stay separate), `script`/`style` text is skipped, and each
        segment is whitespace-normalized (`normalize_ws`). Chapters are segments joined by
        `\n\n` — the frontend then splits on `/\n\s*\n/`, so `para.original` === my normalized
        segment (THE key contract that makes reassembly line up).
      - `assemble_epub(path, output_path, translations: Map<normSrc,translation>)`: re-walks
        each spine XHTML, swaps each translatable run for its translation (preserving the run's
        leading/trailing whitespace), copies ALL other resources (images/CSS/fonts) verbatim,
        writes `mimetype` FIRST + Stored per the OCF spec. Missing-from-map runs (pass-through
        code / untranslated) are kept exactly. Parse error on a doc → that doc is copied
        unchanged (never emit garbage). Output is **translated-only** (see "not done" below).
      - Self-contained entity decoder (`decode_entity` numeric + common named) so what is sent
        to the translator / used as the key is real text; unknown named entities are kept verbatim.
    - Frontend `src/window/Document/index.jsx`: new `fileType` state (`'pdf'|'epub'`); file
      picker + drag-drop accept `.epub`; `handleStartTranslation` branches extraction
      (`extract_epub` vs `extract_pdf_pages`, skipping the PDFium step for EPUB) then shares the
      rest; new `handleExportEpub` (builds the `{normalizedSource: translated}` map from `pages`
      and calls `assemble_epub`); toolbar shows **"Save as EPUB"** for EPUB sources and the
      native-PDF dropdown only for PDF sources; header/landing/viewer labels say PDF/EPUB and
      "Section" vs "Page".
    - Tests: NEW `cargo test document::epub` = **8/8 green** (the 8th is the bilingual transform
      test added with the §2 D follow-up below) — (container parse, spine order by
      itemref, href resolve+percent-decode, entity decode, segmenter skips script/style +
      merges entity runs + preserves inline tags, and a full **in-memory-zip round-trip**:
      build EPUB → extract → assemble → re-open and assert mimetype-first/Stored + translated
      text present). Full `cargo test` = **51/51**, `pnpm test` = **14/14**, `pnpm exec vite
      build` succeeds (Document JSX compiles). Lints clean on `epub.rs` + `index.jsx`.
    - Both former §2 D follow-ups are now DONE (see the next bullet) — there is no longer any
      intentionally-deferred Phase-3 code work, only the ⚠ user GUI verification below.
    - ⚠ VERIFY (user, Windows, real Gemini key): open the Document window, drop a SMALL `.epub`,
      pick engine + target + Batched, Translate (chapters stream like PDF pages), then
      "Save as EPUB" → confirm the output opens in an EPUB reader with translated text, intact
      images/formatting, and untranslated/code paragraphs preserved. Agent can't see the GUI or
      make live API calls.
  - **§2 D follow-ups (bilingual EPUB output + EPUB section labels) — DONE in code + unit-tested;
    bilingual output NEEDS USER GUI VERIFICATION on Windows.** (User asked to continue the list
    2026-05-30 ~04:00 — "do the items in order".)
    - **D(1) Bilingual EPUB output.** `src-tauri/src/document/epub.rs`: `assemble_epub` and
      `transform_xhtml` take a new `bilingual: bool`; `flush_run` gains a bilingual branch. When
      `bilingual` is true a translated run is KEPT and the translation is appended as a sibling
      node — `<br/>` + `<span class="ctranslate-translation">…</span>` — instead of replacing the
      source (a new `write_pieces` helper writes the original run back). Granularity is per text
      RUN (runs split at element boundaries, so inline `<b>/<i>` fragments interleave individually,
      not per whole block) — adequate for the optional mode; true per-block duplication would need
      element-context tracking the streaming pull-parser doesn't keep. `bilingual=false` is the
      exact previous translated-only path (byte-identical). New test
      `transform_bilingual_keeps_original_and_appends_translation` (epub now 8/8).
    - Frontend `src/window/Document/index.jsx`: `handleExportEpub(mode='translated')` now branches
      on `mode` and passes `bilingual` to `invoke('assemble_epub', …)`; output filename gets a
      `_bilingual` suffix in that mode. The "Save as EPUB" button is now a `Dropdown` with
      **"Translated only"** + **"Bilingual (original + translation)"**.
    - **D(2) Cosmetic section labels.** `handleExportMarkdown` + `handleExportHTML` now use
      `fileType==='epub' ? 'Section' : 'Page'` (same as the viewer) and say "EPUB"/"PDF" in the
      title — EPUB exports no longer mislabel chapters as "Page". (The native/print PDF path is
      PDF-only, so its "Page" label is correct and was left alone.)
    - Tests after this work: `cargo test` = **51/51**, `cargo test document::epub` = **8/8**,
      `pnpm test` = **14/14**, `pnpm exec vite build` succeeds. Lints clean on both edited files.
    - ⚠ VERIFY (user, Windows): re-run the EPUB flow, choose "Bilingual" in the Save-as-EPUB
      dropdown, and confirm the output shows the original block followed by its translation with
      intact images/formatting. Agent can't see the GUI.
  - **Phase 4 — Audio/Video → subtitle (HANDOFF §1 table) — increment 1 DONE in code +
    unit-tested; NEEDS USER GUI/NETWORK VERIFICATION on Windows.** (User authorized starting
    Phase 4 2026-05-30 ~04:08.) FILE-based only; Gemini-first (transcription = Gemini audio).
    **No new crates** — reuses `reqwest` blocking (raw-byte upload via the File API, so NO
    base64 and NO ffmpeg; respects the 10 MB rule); audio is NOT split locally — Gemini extracts
    the audio track from video itself.
    - NEW `src-tauri/src/media.rs` (+ `mod media;` in `main.rs`, `use media::*;`, and
      `transcribe_media` + `cancel_transcription` in `invoke_handler`; `subtitle_window` added to
      `window.rs` + `use window::subtitle_window;` + handler). Mirrors `document::translate`
      (background `std::thread`, run-supersede via `ACTIVE_RUN`/`CANCEL_FLAG`, Tauri events).
      Flow in `run_transcription` (CURRENT state — updated by follow-ups #1/#2 below): guess MIME
      from ext (audio+video table; unknown → error) → `media_len_capped` (**2 GB** cap, was 200 MB)
      → **Gemini File API** resumable upload (`upload_file`: start session → stream the file from
      disk in **8 MiB chunks** `X-Goog-Upload-Command: upload`, last chunk `upload, finalize` → poll
      the files endpoint until `state==ACTIVE`) → a **continuation LOOP** of `generateContent` calls
      with `parts:[{text},{file_data:{mime,file_uri}}]`
      and `responseSchema` `[{start,end,text}]` (maxOutputTokens 32768, per-model thinkingConfig)
      → `parse_cues` normalizes timestamps to SECONDS (f64). File-API URLs are derived from the
      resolved generateContent base by `derive_file_api_urls` (`…/v1beta/models/` → upload
      `…/upload/v1beta/files`, files `…/v1beta/files`). Events (all carry `runId`):
      `media://progress {percent,text}`, `media://cues {cues:[{start,end,text}]}`,
      `media://done {status,message,count}`. `parse_timestamp` accepts plain secs / `MM:SS` /
      `HH:MM:SS(.mmm)` / SRT comma; `parse_cues` salvages a MAX_TOKENS-truncated tail (string-
      aware brace matcher) and clamps non-monotonic/zero spans.
    - NEW `src/utils/subtitle.js` (pure SRT/VTT builders + `formatSrtTime`/`formatVttTime`;
      modes `original`|`translated`|`bilingual`, `translated` falls back to original) and NEW
      `src/utils/translationCache.js` (extracted `sha256Hex`/`getEngineModelId`/`get|setCached
      Translation`/`RpmLimiter` — `Document/index.jsx` was intentionally LEFT UNTOUCHED with its
      own copies so the in-flight PDF/EPUB verification isn't disturbed; fold Document onto this
      util later as a separate verified change).
    - NEW `src/window/Subtitle/index.jsx` (route `subtitle:` in `App.jsx`; sidebar button in
      `Config/components/SideBar` invoking `subtitle_window`; i18n `config.subtitle.label` added
      to `en_US`/`tr_TR`). Mirrors the Document window: drop/pick audio/video → **Transcribe**
      (gated to a `geminipro` instance, clear error otherwise; `listen('media://*')` + `invoke
      ('transcribe_media')` + donePromise) → cue list (#, `mm:ss → mm:ss`, original | translation)
      → **Translate Subtitles** REUSES the existing Rust `translate_document` pipeline (dedup +
      cache + structured Gemini batches; same `document://*` events) and fans results back onto
      cues → **Export SRT / Export VTT** dropdowns (Original / Translated / Bilingual) write via
      `writeTextFile`.
    - Tests: `cargo test media` = **13/13 green** (mime, File-API URL derivation incl. proxy
      base, timestamp parsing incl. `fmt_timestamp_hms`, resume-clause prompt, cue sort/dedup/clamp,
      cue parse + truncation salvage + braces-in-strings + empty/bad-start drop, 429 detection,
      thinkingConfig). NEW `src/utils/subtitle.test.js` = **8/8**. Full `cargo test` = **64/64**,
      `pnpm test` = **22/22**, `pnpm exec vite build` succeeds. Lints clean on all new/edited files.
      (2 `cargo` warnings = pre-existing `window.rs` unused imports, not mine.)
    - ALL FOUR Phase-4 follow-ups are now **DONE** in code: (1) long-media continuation, (2) large-
      file chunked upload, (3) non-Gemini cue translation, (4) Document folded onto the cache util —
      see the "Phase 4 follow-ups #1 + #2" and "#3 + #4" bullets below. There is **no remaining
      Phase-4 code work**; only the ⚠ user GUI/network verifications gate the phase.
    - ⚠ VERIFY (user, Windows, real Gemini key): open Config → "Audio / Video Subtitles", drop a
      SMALL audio/video clip, pick a Gemini engine + spoken/target langs, **Transcribe** (progress
      streams, cue list fills with time-coded original text), then **Translate Subtitles** (Gemini
      path fills the right column), then **Export SRT/VTT** (Original/Translated/Bilingual) and
      confirm the file opens correctly in a player (e.g. VLC). Agent can't see the GUI or make live
      API calls.
  - **Phase 4 follow-ups #3 + #4 — DONE in code + frontend-green; #3 NEEDS USER GUI/NETWORK
    VERIFICATION on Windows.** (User authorized continuing the list 2026-05-30 ~10:42 with "I approve
    all changes made so far".) **NO Rust changes — frontend-only.**
    - **#3 Non-Gemini cue translation (in-renderer per-cue).** `src/window/Subtitle/index.jsx`:
      transcription stays Gemini-only, but the TRANSLATE step now accepts ANY builtin engine. New
      `translateEngine` state + a compact engine `Select` in the cue-viewer toolbar (defaults to the
      first configured instance); new `resolveTranslateConfig()` (validates builtin, rejects plugins,
      returns `isGemini`). `handleTranslate` now BRANCHES: `isGemini` → the unchanged Rust
      `translate_document` path (document://* events); else → an in-renderer loop translating each
      UNIQUE cue via `builtinServices[engineName].translate`, paced by `RpmLimiter`, 3× backoff
      retry, per-cue cache write, `applyGlossaryPostTranslate` for non-LLM engines, 429 → stop+cache.
      DEDUP + cache pre-check + FAN OUT are SHARED by both paths. Rate-limit toast made engine-neutral.
    - **#4 Fold `Document/index.jsx` onto `translationCache.js`.** Deleted the 5 duplicated local
      helpers (`sha256Hex`/`getEngineModelId`/`getCachedTranslation`/`setCachedTranslation`/
      `RpmLimiter`) and now `import` them from `src/utils/translationCache.js` (logic was BYTE-
      IDENTICAL → behavior-preserving). `buildNumberedJoin`/`parseNumberedSegments` stay local
      (Document-specific). Cache/RPM helpers are now single-sourced across Document + Subtitle.
    - Tests: `pnpm test` = **22/22**, `pnpm exec vite build` succeeds (Document + Subtitle JSX
      compile). Lints clean on both files. #3/#4 touched NO Rust (the later follow-ups #1/#2 did,
      raising `cargo test` to **64/64**). No new unit tests added by #3/#4 — both are renderer/JSX
      (no new pure-function surface beyond the already-shipping util) and ride the existing GUI
      verification.
    - ⚠ VERIFY (user, Windows): in the Subtitle window, after transcribing, pick a NON-Gemini engine
      (e.g. google/deepl/openai) in the new toolbar engine dropdown and **Translate Subtitles** — the
      right column fills per-cue, cache hits count up, Cancel stops it, Export SRT/VTT still work. The
      Gemini translate path and all Document (PDF/EPUB) flows must still behave exactly as before.
  - **Phase 4 follow-ups #1 + #2 — DONE in code + unit-tested; NEEDS USER GUI/NETWORK VERIFICATION
    on Windows.** (User explicitly chose "do both" at the 2026-05-30 ~11:05 consult.) **Rust-only,
    `src-tauri/src/media.rs`; STILL no new crates / no ffmpeg — respects the 10 MB / no-heavy-dep
    rule.**
    - **#2 Large-file chunked upload.** `read_capped` (whole file → RAM, 200 MB cap) is replaced by
      `media_len_capped` (metadata-only size check, **2 GB** cap, empty-file guard). `upload_file`
      now STREAMS the file from disk in **8 MiB chunks** (`UPLOAD_CHUNK_BYTES`, a 256-KiB multiple per
      the resumable protocol): start session → loop `X-Goog-Upload-Command: upload` + `X-Goog-Upload-
      Offset`, last chunk `upload, finalize` (its response holds the file JSON) → poll PROCESSING→
      ACTIVE (unchanged). Accepts 2xx (+308 for intermediate chunks). Upload progress streams 10→50%.
      So multi-hundred-MB / multi-GB media no longer OOMs or is rejected. New signature takes `&app`,
      `path`, `total_len` (the old `bytes: Vec<u8>` param is gone).
    - **#1 Long-media continuation (ffmpeg-free).** `transcribe_once` now returns
      `TranscribePartial { cues, truncated }` (truncated = finishReason==MAX_TOKENS; empty cues are
      NO LONGER an error). `run_transcription` runs a LOOP: transcribe → merge cues (drop any
      `start < resume_point`) → `sort_dedup_clamp` → `emit_cues` (UI fills live) → if the model
      truncated AND made forward progress AND `rounds < MAX_TRANSCRIBE_ROUNDS` (12), set
      `resume_after = last_end` and call again; `build_transcribe_body` gained a `resume_after`
      param that injects a clause ("continue ONLY after <HH:MM:SS.mmm>, keep timestamps absolute").
      The SAME uploaded file is reused every round — no re-upload, no local audio splitting. Worst
      case (model ignores resume → 0 added / no progress) it just stops with what it had (never worse
      than the old single shot). A continuation-round error keeps the cues already collected; a
      first-round error stays fatal. New helpers: `fmt_timestamp_hms`, `sort_dedup_clamp`.
    - Tests: `cargo test media` = **13/13** (+`fmt_timestamp_hms_formats`,
      `build_body_adds_resume_clause_only_when_resuming`, `sort_dedup_clamp_orders_dedups_and_clamps`);
      full `cargo test` = **64/64**; `pnpm test` = **22/22**; `vite build` ok. Lint clean on
      `media.rs` (the only 2 warnings remain the pre-existing `window.rs` unused imports).
    - ⚠ VERIFY (user, Windows, real Gemini key + a LONG and/or LARGE clip): (a) a clip whose
      transcription previously truncated now continues to the end (cue list keeps growing past the
      old cut-off; progress shows "continuing past …"); (b) a >200 MB file uploads (progress climbs
      10→50%) and transcribes instead of being rejected; (c) Cancel still stops mid-upload and
      mid-continuation. Agent can't see the GUI or make live API calls.
  - **Phase 4 UX — user's FIRST real video-translation test came back UNTRANSLATED (raw Chinese VTT);
    closed the two-step gap. DONE in code + frontend-green; NEEDS USER GUI VERIFICATION on Windows.**
    (User ran their first video translation 2026-05-30 ~11:17; the exported WEBVTT was the raw Gemini
    transcription — clean, well-timed zh cues — with NO translation.) **Frontend-only —
    `src/window/Subtitle/index.jsx`; NO Rust, NO new crates.**
    - Root cause = WORKFLOW, not a bug: transcription is one step, **"Translate Subtitles" is a
      SEPARATE manual step**, and `utils/subtitle.js::cueBody` makes the `translated` export silently
      fall back to original. A first attempt naturally ends at Transcribe → Export = original-only.
      The transcription itself was correct (good zh cues + timestamps; the ~30 s gap at 02:30→03:00 is
      just silence in the clip, not a dropped segment). The Rust `media.rs`/`translate_document` paths
      were NOT touched — they work.
    - **One-click flow.** `handleTranscribe` now takes `{ chain = false }`, captures the final cues in
      a local (`latestCues`), and after a `completed` run (only — not cancelled/rate_limited) `await`s
      `handleTranslate(latestCues, selectedEngine)` — translating with the SAME validated Gemini
      instance used for transcription (the fast batched Rust path). It suppresses the intermediate
      "Transcription complete" toast while chaining.
    - `handleTranslate(cuesArg, engineOverride)` + `resolveTranslateConfig(engineOverride)` now accept
      explicit cues + an engine override (defaults unchanged: `engineOverride || translateEngine ||
      selectedEngine`). The guard + dedup loop use `sourceCues` (the arg, else state `cues`) so the
      chained call isn't blocked by not-yet-flushed React state. Standalone buttons call
      `() => handleTranslate()` so a NextUI PressEvent is never mistaken for a cues array.
    - **UI.** Landing CTA split into primary **"Transcribe & Translate"** (`MdTranslate`, `chain:true`)
      + secondary **"Transcribe only"** (`MdGraphicEq`); landing copy updated. Cue-viewer toolbar now
      shows a warning hint ("Not translated yet — export will be original-only…") whenever
      `!hasTranslation && !busy`, so an original-only export is no longer a silent surprise (the
      Translated/Bilingual export menu items were already disabled until `hasTranslation`).
    - Tests: `pnpm test` = **22/22**, `pnpm exec vite build` succeeds, lints clean on
      `Subtitle/index.jsx`. Renderer/JSX only (no new pure-function surface) → no new unit tests; rides
      the existing Phase-4 GUI verification.
    - ⚠ VERIFY (user, Windows, real Gemini key): reload the same video, pick target lang, click
      **"Transcribe & Translate"** once → cues fill, THEN the right column fills in the same run;
      Export VTT/SRT → Translated/Bilingual now produce real translated text (not the original).
      Also confirm "Transcribe only" still stops at cues and the toolbar warning shows until you
      translate. Agent can't see the GUI or make live API calls.
  - **NEW DIRECTIVE (user spec 2026-05-30 ~12:00) — Task A1: layout-preserving translated PDF —
    DONE in code + unit-tested; NEEDS USER GUI VERIFICATION on Windows.** Strictly ADDITIVE on top
    of the existing PDF flow: the reflow `export_translated_pdf` and `extract_pdf_pages`/
    `render_pdf_page` are byte-for-byte untouched, and this only runs when the user picks the new
    export mode. **NO new crate** — reuses the already-present `pdfium-render` 0.8.37 edit+save API
    (`fonts_mut().load_true_type_from_bytes`, `create_path_object_rect`, `create_text_object`,
    `regenerate_content`, `save_to_file`; all verified to compile).
    - NEW `src-tauri/src/document/pdf_layout.rs` (+ `pub mod pdf_layout;` in `document/mod.rs`,
      `use document::pdf_layout::*;` + `export_layout_pdf` & `cancel_layout_export` in `main.rs`
      invoke_handler). Edits the ORIGINAL PDF IN PLACE: keeps page geometry, embedded images and
      vector graphics; per translated paragraph it (a) locates the source text, (b) draws a WHITE
      redaction rect over the original glyphs, (c) redraws the translation into the SAME bbox with
      shrink-to-fit sizing (`ttf-parser` metrics + bundled DejaVuSans). Runs on a `std::thread`,
      run-supersede/cancel via static `ACTIVE_RUN`/`CANCEL_FLAG` (mirrors `media`/`document::translate`),
      streams `layout_export://progress {runId,percent,text}` + `layout_export://done {runId,status,message}`.
    - **Alignment is EXACT, not fuzzy:** the frontend split each page's `page.text().all()` into
      `paragraphs[].original`; Rust rebuilds that same per-char string from pdfium and `str::find`s
      each `original` (moving cursor handles repeats) → unions the matched chars' `loose_bounds()` →
      paragraph bbox. Pass-through paragraphs (`translated === original`) and empty translations are
      SKIPPED, so the original glyphs stay exactly in place (no silent text loss). CJK *target* output
      is OUT of scope here (DejaVu is Latin/Cyrillic; the reflow export already covers CJK via Noto).
    - Frontend `src/window/Document/index.jsx`: NEW `handleExportLayoutPdf` (listeners→invoke→done-
      promise, mirrors the `translate_document` pattern) + a new "Save as PDF" dropdown item
      `key="layout:preserve"` routed in the existing `onAction` (PDF sources only; EPUB unaffected).
      i18n `config.document.exportLayout` + `exportLayoutDesc` → en_US + tr_TR.
    - Tests: NEW `cargo test document::pdf_layout` = **6/6** (find_run cursor+missing, union_bbox,
      fit_text shrink + floor-never-drops-text, wrap width). Full `cargo test` = **70/70** (was 64),
      `pnpm exec vite build` ok, lints clean on every edited file. (`pnpm test` unchanged at 22/22.)
    - ⚠ VERIFY (user, Windows, real multi-column / image-heavy PDF): translate it, "Save as PDF" →
      "Translated PDF (layout-preserving)", confirm images/vectors/page size are intact, the original
      text is covered, the translation sits in place and shrinks to fit, AND the existing reflow modes
      (Bilingual / Side-by-side / Translated-only / Print) STILL behave exactly as before. Agent can't
      see the GUI.
  - **NEW DIRECTIVE (user spec 2026-05-30 ~12:00) — Task A2: in-image (Chinese) text → OCR → translate
    → overlay — DONE in code + unit-tested; NEEDS USER GUI/NETWORK VERIFICATION on Windows.** Strictly
    ADDITIVE and gated behind a NEW UI toggle ("Translate text in images", default **OFF**): toggle OFF
    ⇒ the layout export sends NO overlays and is byte-identical to A1. **NO new crate** — reuses
    `pdfium-render` 0.8.37's default `image` feature (`get_raw_image`), the already-present `image` 0.25
    + `base64`, and the EXISTING recognize stack (no new OCR engine written).
    - NEW `src-tauri/src/document/image_text.rs` (+ `pub mod image_text;` in `document/mod.rs`,
      `use document::image_text::*;` + `extract_pdf_images` in `main.rs` invoke_handler):
      - `extract_pdf_images(path, page_index) -> Vec<PdfImageInfo>`: iterates `page.objects().iter()`,
        keeps each `as_image_object()`, unions `object.bounds()` → page-space bbox (PDF points), pulls
        the raster via `get_raw_image()`, downscales the longest edge to ≤2000 px (`scaled_dims`), PNG-
        encodes + base64s it. Decorative/icon images are filtered by `keep_image` (≥48×24 pt). A bad
        raster is SKIPPED, never panics (no `unwrap` on user input).
      - `pub struct ImageOverlay { page_index, left, bottom, right, top, translated }` — the translated
        block the layout export draws back. Defined here (the isolated module) and CONSUMED by
        `pdf_layout`.
    - `src-tauri/src/document/pdf_layout.rs` (additive only): `LayoutRequest` gains
      `#[serde(default)] image_overlays: Vec<ImageOverlay>` (absent/empty ⇒ A1 behavior unchanged); in
      the per-page loop, each overlay for that page becomes a `ParagraphEdit` via NEW `plan_overlay_edit`
      (white backing box + shrink-to-fit `fit_text`, reusing A1's exact drawing path `apply_page_edits`).
      No A1 helper was made `pub`; the overlay→edit planning lives inside `pdf_layout`. The drawing is
      the SAME white-redaction + text path as A1.
    - Frontend `src/window/Document/index.jsx`: NEW state `translateImages` (Switch, default off) +
      `ocrEngine` (Select populated from `recognize_service_list`, defaults to first); both shown only
      for PDF sources in the completed-viewer toolbar. NEW `gatherImageOverlays()`: per page →
      `extract_pdf_images` → for each image WRITE it to the cached `pot_screenshot_cut.png`
      (`writeBinaryFile`, `BaseDirectory.AppCache`) so FILE-based recognize engines (system /
      simple_latex / baidu_img) OCR it, AND pass the base64 so base64 engines (tesseract/baidu/…) work →
      `ocrService.recognize(base64, Language.zh_cn, {config})` → offline `CJK_HAN_RE` Han-char gate (only
      Chinese-bearing images get overlaid) → translate via the SAME engine/config/glossary/cache/RpmLimiter
      as body text (`translateImageText`, 3× backoff) → push `{pageIndex,bbox,translated}`.
      `handleExportLayoutPdf` calls it when the toggle is ON and passes `imageOverlays` in the
      `export_layout_pdf` request; OFF ⇒ `[]`.
    - i18n: `config.document.translateImages` / `translateImagesDesc` / `ocrEngine` → en_US + tr_TR.
    - Tests: NEW `cargo test document::image_text` = **4/4** (`keep_image` filter, `scaled_dims` keep/
      shrink/degenerate) + 2 NEW `pdf_layout` tests (`plan_overlay_edit_fits_translation_into_image_box`,
      `plan_overlay_edit_skips_empty_or_degenerate`). Full `cargo test` = **76/76** (was 70),
      `pnpm test` = **22/22**, `pnpm exec vite build` ok, lints clean on every edited file.
    - **Realistic milestone = TEXT-OVERLAY only** (per the brief). The overlay covers the image's bbox
      with a white box + the fitted translation — so an image that is mostly text (diagram/screenshot)
      reads as its translation, but a photo with incidental text gets its whole region whitened. True
      inpaint (cover ONLY the source glyphs, match background) is the documented next step. Because the
      Windows `system_ocr`/recognize services return WHOLE-IMAGE text (no per-line boxes), per-line
      anchoring inside an image isn't available yet — that, too, waits on richer OCR output.
    - ⚠ VERIFY (user, Windows, real Gemini key + a PDF with Chinese text inside images, e.g. a manga page
      or a screenshot-heavy doc): translate it, turn ON "Translate text in images", pick an OCR engine
      that can read Chinese (system needs the zh-CN OCR language pack installed; tesseract auto-downloads
      chi_sim), "Save as PDF" → "Translated PDF (layout-preserving)" → confirm the image's Chinese text
      is overlaid with the translation while body text + layout still behave as A1, AND that with the
      toggle OFF the output is byte-identical to plain A1. Agent can't see the GUI or make live API/OCR calls.
    - **2026-06-18 follow-up (see §1 "BUG FIXES 2026-06-18"):** `gatherImageOverlays` now uses batch
      `scan_pdf_images` + `image_scan://progress` instead of per-page `extract_pdf_images`; PNGs on disk
      via `cachePath`. Layout export progress + `plan_page_edits` perf fix live in `pdf_layout.rs`.
    audio — DONE in code + unit-tested; NEEDS USER GUI/AUDIO/NETWORK VERIFICATION on Windows.**
    (User authorized continuing the directive 2026-05-30 ~12:48 — "continue from where the previous
    agent left off".) Strictly ADDITIVE: the file-based `media.rs` transcribe path
    (`transcribe_media` / `cancel_transcription`) is byte-for-byte untouched; live caption is a
    separate module + commands + overlay window. **§1.6 CRATE DECISION RESOLVED:** the user picked
    **`cpal`** (cross-platform audio; on Windows it auto-enables WASAPI loopback when the default
    OUTPUT device is opened for INPUT). Locked at **cpal 0.15.3** (well under the 10 MB rule; reuses
    windows-rs underneath). Its build artifacts were already cached in `target/` from a prior
    experiment, so the compile stayed fast.
    - NEW `src-tauri/src/media/live_caption.rs` (+ `pub mod live_caption;` INSIDE `media.rs` — Rust
      2018 lets a `media.rs` module own a `media/` submodule dir; `use media::live_caption::*;` in
      `main.rs`; `start_live_caption` + `stop_live_caption` + `live_caption_window` added to
      `invoke_handler`). Mirrors `media`/`document::translate` (background `std::thread`,
      run-supersede/stop via static `ACTIVE_RUN`/`CANCEL_FLAG`, `should_stop`).
    - Flow in `run_session`: `build_loopback_stream` opens the default OUTPUT device for INPUT (cpal
      sets `AUDCLNT_STREAMFLAGS_LOOPBACK`); the cpal data callback (its OWN audio thread) downmixes
      to mono and pushes into a shared `Arc<Mutex<Vec<f32>>>` (capped at `MAX_BUFFERED_SECONDS`=30 s
      to bound RAM). The session thread polls every 200 ms; once `chunk_seconds` (clamped 2–15,
      default 5) of audio is buffered it drains ALL of it (catch-up), `resample_to` 16 kHz
      (box-average decimation = anti-alias low-pass; never upsamples), gates silence by `rms`
      (`SILENCE_RMS`), hand-encodes a 16-bit PCM WAV (`encode_wav_pcm16`), base64s it, and calls
      Gemini `generateContent` with INLINE audio (`inline_data` `audio/wav`, NOT the File API — low
      latency). It emits the recognized text first, then translates it (separate Gemini call;
      `build_glossary_block` mirrors `document::translate`, first-seen wins) and emits again. The
      blocking network runs on the session thread, so capture never stalls (callback is independent).
    - Events (`emit_all`, every carries `runId`): `live_caption { partial, final, translated, ts }`
      and `live_caption://state { state: "listening"|"transcribing"|"stopped"|"error", message }`.
      On stop the cpal `Stream` is dropped (releases the audio device) and `stopped` is emitted.
    - NEW overlay `src/window/LiveCaption/index.jsx` (route `live_caption:` in `App.jsx`; builder
      `live_caption_window()` in `window.rs` — transparent/frameless via `build_window`'s non-mac
      path, `set_always_on_top` + `set_skip_taskbar`, bottom-centre placement; the new label is
      excluded from `set_shadow` so the transparent overlay has no shadow box). Transparent,
      always-on-top, DRAGGABLE (`data-tauri-drag-region`), font +/- and show/hide-original controls,
      and a click-through toggle (`appWindow.setIgnoreCursorEvents`). It only DISPLAYS — listens to
      the events above (accepts all; single session at a time so no runId filter needed) and forces
      a transparent body bg regardless of theme.
    - Controller = the **Subtitle window** (`src/window/Subtitle/index.jsx`, additive only): a NEW
      "Live Captions (system audio)" card in the idle landing resolves the Gemini instance (REUSES
      `resolveGeminiConfig` — transcription stays Gemini-only), fetches glossary, opens the overlay
      (`invoke('live_caption_window')`) and `invoke('start_live_caption', { request })`; Stop calls
      `stop_live_caption` + closes the overlay via `WebviewWindow.getByLabel('live_caption')`.
      Click-through is AUTHORITATIVE here (the overlay can enable it but can't un-toggle itself once
      the mouse passes through): the Switch `emit`s `live_caption://passthrough { enabled }`, which
      the overlay listens for and applies.
    - i18n: NEW `config.subtitle.live.*` (title/desc/target/engine/chunk/chunkDesc/start/stop/
      clickThrough/clickThroughHint/started/listening/transcribing/stopped/error/overlayListening/
      overlayTranslating) → en_US + tr_TR.
    - Tests: NEW `cargo test media::live_caption` = **9/9** (clamp_chunk bounds, stereo→mono downmix
      average, resample decimation halves count + passthrough/no-upsample, RMS silence-vs-signal,
      WAV header bytes + clamping, translation_needed rules, glossary block dedup/escape, quote
      strip). Full `cargo test` = **85/85** (was 76), `pnpm test` = **22/22**, `pnpm exec vite
      build` ok, lints clean on every new/edited file. (The only 2 `cargo` warnings remain the
      pre-existing `window.rs` unused imports.)
    - **Platform note:** capture is Windows-first (WASAPI loopback). cpal compiles cross-platform;
      on macOS opening the output device for input typically can't loopback (needs ScreenCaptureKit —
      future) and on Linux you'd point it at the PulseAudio/PipeWire monitor source — both fail
      gracefully with an `error` state, never a panic. CJK *target* glyphs render fine (the overlay
      is HTML, unlike the DejaVu-limited A1/A2 PDF path). The cpal callback only fires while audio is
      actually playing — so captions naturally pause during silence (expected, not a bug).
    - ⚠ VERIFY (user, Windows, real Gemini key): open Config → "Audio / Video Subtitles", in the new
      "Live Captions (system audio)" card pick a target lang + Gemini engine (+ chunk seconds), click
      **Start Live Caption** while a desktop/browser video plays → the transparent overlay shows the
      recognized text then its translation every ~chunk seconds; **Stop** ends it and leaves the
      audio device clean; the **Click-through** switch makes the bar ignore the mouse (and toggles
      back from the same switch). Confirm the existing file Transcribe / Translate / Export SRT-VTT
      flows STILL behave exactly as before. Agent can't capture audio, see the GUI, or make live API
      calls.
  - **SESSION RE-VERIFICATION (incoming agent, 2026-05-30 ~13:22, UTC+3) — NO code change.** Re-ran the
    full suite on the handed-off tree to confirm nothing regressed before the Phase-5 boundary:
    `cargo test` = **85/85**, `pnpm test` = **22/22** (passthrough 5 + subtitle 8 + glossary 9),
    `pnpm exec vite build` = **ok** (only the pre-existing >500 kB chunk-size warning, NOT an error).
    No files were edited; the directive's A1/A2/B code is intact and green. **PAUSED at the Phase-5
    boundary awaiting the user's go/no-go** (per the user's "notify before Phase 5 and wait for my
    decision"). Still NO commits (user is holding all commits).
  - **BUG FIXES 2026-06-18 (user-reported Windows GUI — Subtitle window + PDF export hangs) — DONE in
    code; NEEDS USER re-verification on Windows. Still NO commits.**
    1. **Subtitle / Document window = empty transparent square behind Config**
       - **Root cause:** `src-tauri/src/window.rs::build_window` applied
         `transparent(true).decorations(false)` to **every** non-macOS window, not just overlay popups.
       - **Fix:** OS-level transparency restricted to labels `translate`, `screenshot`, `live_caption`
         only. Feature windows (`subtitle`, `document`, `config`, `recognize`, `updater`) keep normal
         decorated frames. (`html { background: transparent }` in `style.css` is unchanged — only
         overlay windows rely on it.)
    2. **PDF translation / export stuck at PDFium init (never progresses past toast)**
       - **Root cause:** `src-tauri/src/document/pdf.rs` downloaded `chromium/7843` from
         bblanchon/pdfium-binaries, but `pdfium-render 0.8.37` expects **`pdfium_7543`**
         (`chromium/7543`). Wrong ABI: bind may succeed but subsequent Pdfium calls hang or crash.
       - **Fix:** `const PDFIUM_RELEASE: &str = "chromium/7543"`. Sidecar tag file
         `{config_dir}/com.pot-app.desktop/pdfium.release` written on download; if tag missing or
         mismatched, stale `pdfium.dll` is deleted and the correct release is re-fetched. URL built from
         `PDFIUM_RELEASE.replace('/', "%2F")`.
       - **User cleanup (one-time, if an old wrong DLL was cached before this fix):** delete
         `%APPDATA%\com.pot-app.desktop\pdfium.dll` **and** `pdfium.release` — next run re-downloads 7543
         (~5–6 MB).
    3. **Layout PDF export stuck at "Scanning images on page X/Y…" (A2, "Translate text in images" ON)**
       - **Root cause:** `Document/index.jsx::gatherImageOverlays` invoked `extract_pdf_images` **once
         per page** — each call re-loaded the entire PDF and returned huge base64 blobs over IPC;
         `get_raw_image()` on pathological rasters could block indefinitely with no progress events.
       - **Fix (Rust):** NEW command `scan_pdf_images(path, page_count)` in
         `src-tauri/src/document/image_text.rs`. Single Pdfium load; iterates pages; writes PNGs to
         `{cache}/com.pot-app.desktop/pdf_image_scan/`; returns `PdfImageInfo` with `pageIndex` +
         `cachePath` (no base64). Emits `image_scan://progress { pageIndex, pageCount, text }` per page.
         Caps: `MAX_IMAGES_PER_PAGE = 48`; worker thread + overall timeout (`page_count × 90s`, max 30 min).
         Legacy `extract_pdf_images(path, pageIndex)` kept (thin wrapper) for compat/tests.
       - **Fix (frontend):** `gatherImageOverlays` listens for `image_scan://progress`, calls
         `scan_pdf_images` once, groups by page, reads PNGs from `cachePath` (writes
         `pot_screenshot_cut.png` for file-based OCR engines; `convertFileSrc` for tesseract). OCR wrapped
         in 45 s timeout per image (`ocrWithTimeout`).
       - **Wired:** `scan_pdf_images` added to `main.rs` `invoke_handler` (alongside `extract_pdf_images`).
    4. **Layout PDF export stuck at "Initializing PDFium…" (toast never updates — misleading label)**
       - **Root cause:** Toast showed the **first** `layout_export://progress` text and froze there while
         `pdf_layout.rs::plan_page_edits` scanned **every character** on the page via
         `loose_bounds()` before emitting the next event. Dense CJK PDFs (user's robot-manual doc) took
         minutes with no UI feedback.
       - **Fix:** `assemble()` now emits granular steps: `Preparing PDFium…` → `Binding PDFium…` →
         `Opening source PDF…` → `Loading fonts…` → per-page `Analyzing page X/Y…` / `Matching translated
         block …` / `Applying translation on page X/Y…` / `Finished page X/Y`.
       - **Perf refactor:** `plan_page_edits` fast path uses `page.text().all()` + production `find_run()`
         (byte-offset search, same contract as frontend `splitPdfSourceParagraphs` source). Bbox union +
         font-size estimate call `loose_bounds()` **only for the matched char range**, not the whole page.
         Normalized fallback (`PageGeometry` slow path) builds the old matchable map **only when direct
         find fails** for a paragraph, with progress callbacks during long glyph reads.
       - **Tests:** `cargo test document::pdf_layout` still **13/13** green after refactor.
    - **Files touched:** `src-tauri/src/window.rs`, `src-tauri/src/document/pdf.rs`,
      `src-tauri/src/document/image_text.rs`, `src-tauri/src/document/pdf_layout.rs`, `src-tauri/src/main.rs`,
      `src/window/Document/index.jsx`.
  - **NEW DIRECTIVE (user, 2026-08-15) — external image-translation SERVICE integration — DONE in
    code + unit-tested; NEEDS USER GUI/NETWORK VERIFICATION on Windows (service must be running).**
    Integrates the user's standalone **layout-image-translator** FastAPI microservice (PaddleOCR-VL,
    separate repo at `C:\Users\enesk\OneDrive\Masaüstü\OCR Project`) as an ALTERNATIVE in-image
    translation method for the layout-preserving PDF export. The service receives an embedded PDF
    image (`POST {base}/v1/translate`, multipart `image`+`source_lang`+`target_lang`) and returns the
    SAME image with its text translated in place (layout preserved, identical pixel dimensions) — so
    drawing it back at the original image's exact bbox keeps the page layout 1:1. **The service repo
    is NEVER modified or bundled — consumed over HTTP only** (localhost default, remote URL works for
    a future mobile/hosted setup). Strictly ADDITIVE: the A2 OCR+overlay path stays the DEFAULT and
    is byte-for-byte untouched; the service path only runs when the user picks it.
    - NEW `src-tauri/src/document/image_service.rs` (+ `pub mod image_service;` in `document/mod.rs`,
      `use document::image_service::*;` + `check_image_service`, `translate_image_via_service` in
      `main.rs` invoke_handler): `check_image_service(base_url)` = 4s GET `{base}/openapi.json` probe
      (returns bool, never errors); `translate_image_via_service(base_url, cache_path, source_lang,
      target_lang)` = reqwest blocking multipart POST (300s timeout, `.no_proxy()`), saves the
      returned image to `{cache}/com.pot-app.desktop/pdf_image_service/<same-name>.png`, returns the
      path. PNG/JPEG magic-byte + content-type sniff (`looks_like_image`) rejects JSON error bodies.
      `pub struct ImageReplacement { page_index, left, bottom, right, top, image_path }` defined here,
      consumed by `pdf_layout`. **Cargo.toml delta: reqwest gains the `multipart` feature** (no new
      crate).
    - `src-tauri/src/document/pdf_layout.rs` (additive only): `LayoutRequest` gains `#[serde(default)]
      image_replacements: Vec<ImageReplacement>` (absent/empty ⇒ prior behavior byte-identical); in
      the per-page loop each replacement is drawn via `apply_image_replacements` → pdfium
      `create_image_object(left, bottom, &img, Some(w), Some(h))` BEFORE the text edits (body-text
      redactions paint on top). `replacement_rect` validates geometry (degenerate/pathless ⇒ skipped,
      original image kept). Best-effort per image — a bad file never fails the export.
    - NEW `src/utils/imageService.js` (+ `imageService.test.js`, vitest): `serviceLangName(code)`
      maps Pot lang codes → the service's language names ("tr"→"Turkish", "auto"→"auto"; unknown
      codes pass through); `normalizeServiceUrl(url)` trims, strips trailing slashes, defaults scheme
      to http:// and falls back to `DEFAULT_IMAGE_SERVICE_URL` (`http://127.0.0.1:8765`).
    - Frontend `src/window/Document/index.jsx` (additive): NEW state `imageMethod` (`'ocr'` default |
      `'service'`) + persisted config `image_translator_service_url` (useConfig). NEW
      `gatherImageReplacements()`: health-check (friendly "start the service" error if down) →
      `scan_pdf_images` (existing batch scan + progress events, unchanged) → per image
      `translate_image_via_service` → `[{pageIndex, bbox, imagePath}]`; failed images are skipped
      (original kept). `handleExportLayoutPdf` branches on `imageMethod` and passes
      `imageReplacements` alongside `imageOverlays`. UI: method Select next to the "Translate text in
      images" Switch; `'ocr'` shows the existing OCR-engine Select, `'service'` shows a URL Input.
    - i18n: `config.document.imageMethod/imageMethodOcr/imageMethodService/imageServiceUrl/
      imageServiceUrlDesc/imageServiceDown/imageServiceProgress` → en_US + tr_TR.
    - Tests: NEW `cargo test document::image_service` = 4/4 (endpoint normalization, localhost
      fallback, image sniff, camelCase deserialize) + 4 NEW `pdf_layout` tests (`replacement_rect`
      valid/degenerate, `LayoutRequest` serde-default without/with `imageReplacements`). NEW vitest
      `src/utils/imageService.test.js`.
    - ⚠ VERIFY (user, Windows): start the service (`image-translator serve --host 127.0.0.1 --port
      8765` in the OCR Project venv, or docker compose up), open Document, translate an image-heavy
      PDF, set method to "Image service (PaddleOCR-VL)", "Save as PDF" → layout mode → confirm each
      embedded image is replaced by its translated version at the exact original position, body text
      still behaves as A1, and with method 'ocr' (or service down + friendly error) everything
      behaves exactly as before. Agent can't see the GUI or reach the local service.
    - **Service setup (learned the hard way 2026-08-15):** the venv needs `pip install -e
      ".[paddleocr-vl]"` **plus `torchvision`** (missing from the extra — `AutoImageProcessor
      requires the Torchvision library`), and for the user's RTX 4070: `pip install --force-reinstall
      --no-deps torch torchvision --index-url https://download.pytorch.org/whl/cu128` (plain
      `--upgrade` is a no-op because the +cpu wheel has the same version) + `PADDLEOCR_VL_DEVICE=gpu:0`
      in `.env`. ~3.7 s/image on GPU vs minutes on CPU. The service does NOT read `.env` itself —
      launch it with the added `start-image-service.cmd` in the OCR Project root, and start it
      DETACHED (it dies with the agent session otherwise, which silently produced an
      images-untranslated export once). `document_image_method` config persists the method choice;
      default is now `'service'`.
  - **USER DIRECTIVE (2026-08-15) — preserve SOURCE TEXT ATTRIBUTES in the layout PDF export — DONE
    in code + unit-tested; NEEDS USER GUI VERIFICATION on Windows.** User's export showed wildly
    inconsistent type: headings shrank, and wrapped bullet tails ("uzun araçlar için).") rendered at
    ~4 pt next to full-size lines. Rewrite of `plan_page_edits` in `src-tauri/src/document/pdf_layout.rs`
    (drawing path + commands + request shape unchanged).
    - **Root causes.** (1) Font size was *inferred from glyph bounding-box heights*, not read from
      the PDF text state. (2) `expand_layout_paragraphs` split every multi-line block into per-line
      paragraphs (needed for tables) and never rejoined them, so a wrapped tail got its own narrow
      bbox and `fit_text` shrank its translation to fit — floor was the absolute `MIN_FONT_PT` 4 pt.
      (3) Turkish runs much longer than Chinese, and the box was capped at the *source glyph* width,
      so even a heading had to shrink.
    - **Real attributes, one pass.** NEW `scan_char_range` → `CharScan { positions, size, color,
      bold }` replaces the separate `union_bbox_for_char_range` / `estimate_font_size_for_char_range`
      / `split_bbox_by_rows` passes (fewer `loose_bounds()` calls than before, so this is also a perf
      win). Size = median of per-char `scaled_font_size()`. Colour = first non-near-white
      `fill_color()` (white-on-white would vanish against the white redaction box → falls back to
      BLACK). Bold = majority of `font_weight() >= 600` or a "bold" font name, sampled from at most
      `STYLE_SAMPLE_CHARS` (24) chars. Applied in `apply_text_style` via `set_fill_color`, and for
      bold `set_stroke_color`/`set_stroke_width`/`set_render_mode(FilledThenStroked)` — an outline
      stroke emulates bold because DejaVuSans ships no bold variant (no new font asset, no size hit).
    - **Regrouping wrapped lines.** `expand_layout_paragraphs` now returns an `Expansion { lines
      (each tagged orig_idx + line_idx), full, full_source, per_line }`. NEW `group_wrapped_lines` +
      `is_wrapped_continuation` rejoin consecutive same-paragraph lines. The decisive signal is that
      the previous line **reaches the page's text column edge** — a line only wraps because it ran
      out of room; table rows stop well short. Guards: same paragraph, consecutive line_idx, next
      line on the following baseline, matching left margin, comparable size, and pieces of a
      multi-row (table) match are marked `groupable: false` and never merged. A group covering ALL
      of a paragraph's matched lines is redrawn from the WHOLE paragraph translation; partial groups
      fall back to the per-line slices (previous behaviour).
    - **Room to keep the size.** NEW `grow_layout_box` grows the *layout* box rightwards to the
      column edge and down by up to 2 lines, stopped by any other matched block sharing that band.
      `ParagraphEdit` now carries BOTH `bbox` (layout) and `redact` (white box) — **the redaction
      still covers only the original text union**, so growing the layout box can never white out
      content that was not verified free. `fit_text` starts at the source size and floors at
      `MIN_SHRINK_RATIO` (0.72) of it instead of 4 pt.
    - Tests: `cargo test document::pdf_layout` = **26/26** (6 new: wrapped-tail regrouping, table
      rows NOT regrouped, distant/foreign/ungroupable lines rejected, source size kept when it fits,
      shrink floor honoured, layout box grows only into free space). `fit_text_floor_never_drops_text`
      updated — it asserted the old absolute-4pt floor this change deliberately replaces. Full
      `cargo test` = **107/107** (`-- --skip inspect_pdf_text`; that pre-existing debug test binds
      pdfium against a Desktop PDF and hangs).
    - ⚠ VERIFY (user, Windows): re-export the same manual in layout mode and confirm headings keep
      their size, wrapped bullet tails are no longer tiny, bold/coloured source text stays bold and
      coloured, and TABLES still get one translation per cell (the regrouping must not glue rows).
  - **FOLLOW-UP (2026-08-15, same directive) — the regrouping never fired; alignment + uniform sizing
    added — DONE in code + unit-tested + validated against the real PDFs; NEEDS USER GUI VERIFICATION.**
    User's next export still showed one paragraph rendered part-large/part-small and a page title that
    wrapped and lost its centring.
    - **Root cause (measured, not guessed).** `src/utils/pdfParagraphs.js::splitPdfSourceParagraphs`
      splits EVERY multi-line block into **one translation unit per source line**. So the previous
      commit's `is_wrapped_continuation` — which required `prev.orig_idx == next.orig_idx &&
      line_idx+1` — could never fire on a real document: each wrapped line arrives as its own unit.
      Every line was therefore fitted into its own line box independently, which is exactly why sizes
      varied line by line. Verified by dumping real per-line geometry with pypdfium2 (see the
      scratchpad recipe in the bullet below).
    - **Geometric grouping.** `is_wrapped_continuation` no longer looks at translation-unit identity
      at all; it groups on geometry only (comparable size, next baseline, same left margin ±3×size,
      `groupable`, and the decisive test: `prev` reaches the page text column's right edge — a line
      only wraps because it ran out of room). `recs` are sorted into reading order first.
      `group_text` resolves a group's text: whole-unit translation when the group covers all matched
      lines of one unit, else the members' per-line/single-line translations joined.
    - **Alignment preservation.** NEW `TextColumn { left, right }` + `Align { Left, Center }` and
      `line_start_x`. Multi-line blocks prove centring by sharing a centre axis while their left edges
      differ; single lines need generous, near-equal margins (>4×size, ≤15% imbalance). Centred blocks
      also grow symmetrically in `grow_layout_box` (keeping their centre fixed), which is what lets a
      longer translated page title stay on ONE centred line instead of wrapping.
      **There is deliberately NO `Align::Right`** — a margin-based rule classified left-flush cells in
      a table's right-hand column as right-aligned and would have shoved their text to the cell edge.
      Caught by simulating the algorithm on the user's real table page before shipping.
    - **Uniform sizing.** NEW `unify_sizes`: blocks sharing a source font size (±0.5 pt) are all drawn
      at the smallest size any of them needed, then re-wrapped. Shrinking a block that already fit is
      always safe, so this cannot overflow, and it removes the part-large/part-small effect entirely.
      Bounded by the existing 0.72 floor, so a page can never collapse below 72% of source size.
      `ParagraphEdit` gained `align`, `source_size` and `text` (the last so blocks can be re-wrapped).
    - **Validation beyond unit tests.** `scratchpad/inspect_lines.py` + `simulate_grouping.py` (Python
      + the OCR venv's pypdfium2) port the grouping/alignment rules and run them on real PDFs — reuse
      them before changing these heuristics. Charger manual p1: 31 lines → 17 blocks, every paragraph
      regrouped, headings NOT absorbed, title + footer `Center`, body `Left`. API guide p20 (dense
      table): all 15 blocks `Left`, cells not glued across rows.
    - Tests: `cargo test document::pdf_layout` = **33/33** (new: regrouping across translation units,
      heading not absorbed by the paragraph below, hanging-indent limit, table cell in the right-hand
      column stays left-aligned, multi-line centring evidence, centred block grows symmetrically,
      `unify_sizes` one-size-per-class). Full `cargo test` = **114/114** (`-- --skip inspect_pdf_text`).
    - ⚠ VERIFY (user, Windows): re-export the charger manual — the page title should be a single
      centred line, each paragraph should be ONE consistent size, and the API guide's tables should
      still read cell-by-cell with left-aligned text.
  - **FOLLOW-UP 2 (2026-08-15) — TOC / table / backdrop defects from the user's 25-page charger
    manual export — DONE in code + unit-tested + re-validated on the real PDFs; NEEDS USER GUI
    VERIFICATION.** User reported: contents page redrawn as one run-on paragraph, page title wrapped
    to two lines on that page, table cell text spilling across cell borders, and shaded table header
    cells turning white. All four were regressions/limits of the geometric layout work above.
    - **Contents page (root cause).** Every TOC row ends in a dot leader that reaches the column's
      right edge — the exact signal `is_wrapped_continuation` uses for "this line wrapped". Measured
      with `simulate_grouping.py`: **28 TOC rows chained into 1 block.** Fixed with
      `looks_like_toc_line` (3+ consecutive leader chars: `. · ⋅ … ・ ‧`) stored on `LineRec.is_toc`;
      a TOC row never starts or continues a group. Re-measured: **31 rows → 31 blocks.**
    - **Page title wrapped (root cause).** Alignment was measured against the page's *text column*,
      but a contents page is indented (column left 92.7 instead of 56.7), which skewed the margins so
      the centred title read as `Left`, grew only rightwards, and wrapped. `TextColumn` now also
      carries the **physical page centre**; a single-line block is `Center` when its own centre sits
      on the page centre (±2% of page width) with real margins on both sides. Re-measured: the title
      and the "目录" heading are both `Center` again, and figure captions ("图3-1") now centre too.
    - **Table cells spilling (root cause: invisible obstacles).** Two gaps. (a) Lines with NO
      translation (pass-through numbers like "410±2mm") were skipped entirely in phase 1, so they did
      not exist as obstacles and a neighbouring cell grew straight over them — they are now located
      and kept as `LineRec { drawable: false }`, drawn never, but blocking always. (b) Nothing
      accounted for the *drawn* geometry: NEW `collect_obstacles` gathers the bounds of every
      non-text page object (cell rules, callout borders, images). In `grow_layout_box` a box that
      **encloses** the block clamps growth inside it (a cell's rules keep the cell's text in the
      cell), while one beside/below it blocks growth at its edge (`encloses` / `vertical_overlap` /
      `horizontal_overlap` helpers). This also stops body copy growing into the "说明" callout below
      it — the images-5/6 overlap the user asked to be analysed rather than fixed.
    - **Shaded headers turning white (root cause).** The redaction box was hard-coded
      `PdfColor::WHITE`, which erased the grey fill behind a table header. NEW `PageBackdrop` renders
      the page once (`render_with_config`, `BACKDROP_RENDER_SCALE` = 1.0 px/pt), and each edit's
      redaction is filled with the **dominant colour inside its own box** (RGB quantised to 5 bits
      per channel, mode via HashMap — glyphs are a minority of the area, so the mode is the backdrop).
      Falls back to white when the render fails.
    - **Cells no longer drag the page down.** A block narrower than `NARROW_BOX_RATIO` (0.55) of the
      column is treated as a cell/label: it may shrink to `MIN_SHRINK_RATIO_NARROW` (0.5) to stay
      inside its rules, and it is excluded from `unify_sizes` **on both sides** — one cramped cell no
      longer shrinks a whole page of body copy, and body copy no longer forces a cell to overflow.
      `fit_text` gained a `floor_ratio` parameter for this.
    - Tests: `cargo test document::pdf_layout` = **40/40** (7 new: TOC rows never chain, leader
      detection incl. negatives, untranslated line still blocks growth, enclosing box clamps growth
      inside a cell, a box below blocks downward growth, `encloses` container-vs-neighbour, narrow
      cells excluded from unification). Full `cargo test` = **121/121**.
    - Re-validated with the scratchpad simulators on the user's real files: contents page 31→31
      blocks, body page unchanged at 17 blocks with paragraphs still merged, table page groups
      bullets correctly and centres the figure caption.
    - ⚠ VERIFY (user, Windows): re-export the charger manual — contents page reads one entry per
      line, the title is a single centred line, table text stays inside its cell, and the grey header
      row of the "内容/级别" table is still grey.
    - **User confirmed 2026-08-15:** cell containment and the page-overlap defect (images 5/6) are
      fixed. Two NEW defects from this same follow-up were reported and fixed below.
  - **FOLLOW-UP 3 (2026-08-15) — black redaction boxes + table typography — DONE in code +
    unit-tested; NEEDS USER GUI VERIFICATION.**
    - **Black boxes over headings/table headers (root cause).** `PageBackdrop::dominant_color` took
      the plain mode of the pixels inside a block. Dense CJK ink — bold headings, table header rows —
      covers MORE than half its own box, so the mode was the INK and the redaction box was painted
      black straight over the text it was meant to hide (the translation, drawn on top in the same
      dark colour, vanished into it). Renamed to `background_color` and reworked: pixels within
      `INK_EXCLUSION_DISTANCE` (120 in RGB space) of the block's own text colour are discarded first,
      the mode is taken over what remains, and the winner is rejected unless it clears
      `MIN_BACKDROP_CONTRAST` (90) against that ink — the source text was legible, so its backdrop
      must contrast with it. Rejection falls back to white. Pure helper `background_from_samples` is
      unit-tested (70%-ink heading → white; 60%-ink grey header → grey; all-ink → None; light text on
      a dark panel → the dark panel).
    - **Table text jumping between sizes (root cause).** FOLLOW-UP 2 excluded narrow blocks from
      `unify_sizes` entirely to stop one cramped cell shrinking a page of body copy — but that left
      every cell sized on its own, so lines within a single cell rendered at wildly different sizes
      and spilled. `unify_sizes` now keys its classes on `(source size, narrow)`: cells unify with
      CELLS and body copy with BODY. Both goals hold at once — the typography preservation the rest
      of the document gets now applies inside tables too, without cells and body dragging each other.
    - Tests: `cargo test document::pdf_layout` = **43/43** (4 new). Full `cargo test` = **124/124**.
    - ⚠ VERIFY (user, Windows): re-export — no black patches over headings or table header rows
      (shaded headers still shaded), and table cell text at one consistent size per source size.
  - **FOLLOW-UP 4 (2026-08-23) — table cells merged into ONE translation unit — DONE in code +
    unit-tested + measured on the user's PDF; NEEDS USER GUI VERIFICATION.** User's spec-table export
    showed a cell's text spilling into its neighbour, the next column left EMPTY, and those blocks
    rendered far too large.
    - **Root cause is in EXTRACTION, not layout** (`src-tauri/src/document/pdf.rs::
      extract_page_text_with_column_split`). Its column split only fires for characters that share a
      baseline (`vertical_diff < font_height * 0.4`). In a table each cell centres its own content,
      so neighbouring cells in one row routinely sit half a line apart. Measured on page 20 of the
      charger manual: `'输入单相三线'` ends at x=208.4 (y-centre 602.9) and `'4000W'` starts at
      x=231.9 (y-centre 608.0) — **|dy| = 5.1 vs a 3.6 threshold**, so the same-line test failed, the
      23.5 pt column gutter was never examined, and the two cells became the single unit
      `'输入单相三线 4000W'`. That one unit then (a) drew both cells' translation inside the first
      cell, (b) left the second cell blank, and (c) spanned two cells so its box was not `narrow` —
      it got unified with BODY copy and rendered oversized.
    - **Fix: use the drawn table rules.** NEW `collect_column_rules` gathers vertical rule positions
      from the page's PATH objects (thin tall paths contribute their centre; wider filled shapes
      their left/right edges — a shaded header cell's edges are cell boundaries too), and
      `has_rule_between` splits whenever a rule falls in the gap between two characters, **regardless
      of baseline**. The old gap heuristic is kept as an OR for tables drawn without rules. A minimum
      gap (`MIN_RULE_GAP` = 2 pt) stops a rule that merely passes near a word from breaking it.
    - Measured before/after on the real file: page 20 (the spec table) **105 → 117 units**, with
      `'输入单相三线 4000W'`, `'国标 10A 插座 48V30A'` and 10 more cross-cell units now split per cell.
      Prose pages are untouched — pages 1, 2, 8 have no rules at all (32→32, 31→31, 15→15 units) and
      page 12 has 6 rules but the same 11 units, so there is no over-splitting.
    - **Wrapping for CJK→Latin cells.** `wrap_paragraph` now also breaks after `/ - ， 、 · \ |`
      (`break_opportunities`), because a translated cell value often has no spaces to wrap at and was
      being hard-broken mid-token or needlessly shrunk. Each piece records whether a space preceded
      it, so rejoining pieces on one line cannot invent a space inside "220Vac/20A" — covered by a
      test that asserts the concatenated lines equal the source string exactly.
    - **NOTE for the user:** extraction output changed, so the affected paragraphs have new cache
      keys and WILL be re-translated (quota cost) on the next run. Prose pages keep their cache.
    - Tests: 4 new in `document::pdf` (rule between cells with differing baselines — the measured
      case; rule outside the gap; a rule cannot break a word; no rules ⇒ no split) + 2 new wrap tests.
      Full `cargo test` = **130/130**.
    - ⚠ VERIFY (user, Windows): re-export the spec table — each cell's translation inside its own
      cell, the 输入功率 / "Giriş gücü" column populated again, and cell text at a consistent size.
  - **FOLLOW-UP 5 (2026-08-23) — one translation unit per CELL — DONE in code + unit-tested +
    measured; NEEDS USER GUI VERIFICATION.** After FOLLOW-UP 4 the user reported two leftovers: a
    giant "Yan şarj 220Vac, 20A" drawn across the table, and a cell rendered as overlapping fragments
    ("Şarj / Elektri / 柜 :4000 / W"). Both were extraction defects, measured on pages 20–21.
    - **Under-split — the backwards jump.** `'侧充 220Vac，20A'`: the 位置 column ends at x=455.9 and
      the NEXT ROW's first cell starts at x=154.1, i.e. a **-301.8 pt** gap. Both the rule test and
      the gap test require a positive gap, so neither fired and two cells merged. A backwards jump
      larger than a line height is now a block break.
    - **Over-split — spread-out cells.** A narrow cell sets its text spread out: measured `'充'`→
      `'电'` on the SAME baseline **20.9 pt apart against a 13.7 pt threshold**, so the gap heuristic
      split inside a word; and the cell's wrapped lines jumped back to the cell's left edge, which
      the new backwards-jump rule then split again. Result was one translation unit per fragment,
      each translated and drawn separately — the overlapping mess. Both tests are now **gated on
      `in_same_cell`**: inside one cell the rules already define the boundary, so neither a wide gap
      nor a backwards jump starts a new block. Pdfium's own line breaks are likewise held
      (`pending_newline`) and turned into a SPACE when the next glyph is in the same cell.
    - Whitespace no longer takes part in any of these decisions (spaces are reported with zero
      bounds, which read as a huge backwards jump and split cells mid-word).
    - The decision is now the pure, testable `is_block_break(rules, prev, cur)` + `cell_span` /
      `in_same_cell`. **Cells are the organising principle**: rules delimit, everything inside is one
      block, and with NO rules on the page every heuristic falls back to the original behaviour.
    - Measured on the user's file: page 21 **105 → 54 units** — each cell is now exactly one unit
      (`'充电 柜 :4000 W 伸 出 机 构:500W'`, `'1、4mm²多股电源线 (推荐颜色…) 2、空开选择≥25A 3、国标 10A
      插座'`); page 20 has **0** remaining cross-cell merges; prose pages 1/2/8 unchanged at
      32/31/15 units.
    - Tests: 5 new in `document::pdf`, each built from the measured coordinates above (rule between
      cells on differing baselines; spread-out cell not broken; wrap inside a cell not a new block;
      new table row breaks on the backwards jump; no rules ⇒ original column heuristic intact).
      Full `cargo test` = **138/138**, zero warnings, `pnpm test` 43/43.
    - **NOTE:** table paragraphs change again, so those cells re-translate once (quota). Prose keeps
      its cache. The upside is the model now sees a whole cell instead of fragments, so cell wording
      should improve as well as the layout.
    - ⚠ VERIFY (user, Windows): re-export both table pages — no oversized text spanning columns, no
      overlapping fragments in narrow cells, and body/prose pages unchanged.
  - **FOLLOW-UP 6 (2026-08-23) — layout export froze at "Matching translated block N of M" — FIXED;
    NEEDS USER GUI VERIFICATION.** User's export hung forever on that toast. Measured first: the pot
    process used **0.00 CPU seconds over 5 s wall**, so it was not looping — the export THREAD had
    died and no `done` event was ever emitted, leaving the window on its last progress message.
    - **Root cause — byte offsets used as character indices** (`PageGeometry::locate` in
      `pdf_layout.rs`). `str::find` returns BYTE offsets, and `end = start + needle.len()` is also in
      bytes, but `self.matchable` / `self.super_matchable` are `Vec<MatchableChar>` indexed per
      CHARACTER. On CJK text (3 bytes per glyph) the byte offset runs ~3× past the vector's length,
      so `self.matchable[start]` panicked with an out-of-bounds index. Latent for a long time: the
      slow path only ran when `find_run` missed. **FOLLOW-UP 5 made it fire** — cell text now joins
      its wrapped lines with SPACES while pdfium's raw `text.all()` still has the newlines, so every
      table cell misses the fast path and goes through `locate`.
    - **Fix:** NEW `original_idx_range` + `char_count_before` convert the byte range to character
      indices and use `.get()`, returning None instead of panicking on any mismatch.
    - **Fix 2 (why it was invisible):** the export thread now runs inside `catch_unwind`, so a panic
      is reported as `layout_export://done { status: "error" }` with the panic message instead of
      hanging the window forever. Any future panic surfaces as an error toast.
    - Tests: 3 new (`locate_maps_byte_offsets_to_character_indices` — CJK page, asserts CHARACTER
      indices; miss/empty returns None; the cursor advances across repeats). The first would have
      panicked on the old code. Full `cargo test` = **141/141**, zero warnings.
    - ⚠ VERIFY (user, Windows): close the stuck window, restart, and re-export — it should complete
      (or show a real error) rather than sitting on a progress message.
  - **FOLLOW-UP 7 (2026-08-23) — the same sentence redrawn dozens of times — FIXED; NEEDS USER GUI
    VERIFICATION.** After FOLLOW-UP 5/6 the user's export repeated one translated sentence down a
    whole page.
    - **Root cause — a tall cell's translation handed to every row box.** FOLLOW-UP 5 correctly joins
      a cell's wrapped lines into ONE translation unit. In layout that unit's match then covers
      several visual rows, `CharScan::split_rows` split it into N boxes (right for a run crossing
      table cells, wrong here), and `group_text`'s fallback gave **the whole unit's translation to
      every one of those boxes**.
    - **Fix 1 — do not split what is one cell.** `collect_obstacles` now runs before Phase 1, and a
      multi-row match whose rows all sit inside the SAME drawn box is kept as a single block (NEW
      `share_one_container` / `smallest_container`). Rows in different cells still split.
    - **Fix 2 — the invariant, as a safety net.** NEW `Placed` replaces the ad-hoc lookups:
      **every source line's translation may be placed exactly once per page** (`take_whole` /
      `take_line`, with a whole placement suppressing later per-line ones). Even if some other path
      resolves twice to the same unit, the text can no longer repeat.
    - **Fix 3 — page frames are not cells.** `in_same_cell` now rejects a rule-bracketed span wider
      than `MAX_CELL_WIDTH_RATIO` (0.6) of the page: a frame drawn around the whole text area would
      otherwise bracket every line and collapse a page into one unit. Verified by disabling the guard
      on other manuals (no collapse there — this one is a safety measure, not the trigger).
    - Tests: 4 new (wrapped lines of one cell stay one block; rows in different cells still split, and
      no geometry ⇒ split stands; a unit's translation is drawn only once; per-line slices still fill
      every line). Full `cargo test` = **145/145**, zero warnings.
    - ⚠ VERIFY (user, Windows): re-export the parameter manual — each paragraph appears once, and
      tall table cells read as one flowing block.
  - **FOLLOW-UP 8 (2026-08-23) — untranslated Chinese left in cells + a wrapped heading — FIXED;
    NEEDS USER GUI VERIFICATION.** User reported the rest of the document is now correct, with two
    small defects left.
    - **Untranslated source left visible in table cells (root cause).** Measured on page 27 of the
      parameter manual: extraction is right — the cell is ONE unit
      (`'反光条上线：关 里程计融合：关 高反场景：开 原地上线：开'`). But its match covers 4 visual
      rows, `split_rows` split it into 4 boxes, and FOLLOW-UP 7's "draw once" rule then gave the
      first box the whole translation and the others nothing. **A box with no text produces no edit
      — and therefore no redaction** — so the original Chinese stayed visible under the translated
      first line. FOLLOW-UP 7's `share_one_container` did not save it because this table's cells are
      drawn as separate segments, so no single box encloses the rows.
    - **Fix: splitting is now the exception.** Cells are separated at EXTRACTION (FOLLOW-UP 4/5), so
      one unit spanning several rows is almost always wrapped text. NEW `rows_need_splitting` /
      `rows_are_separated` keep the split ONLY when consecutive rows sit in different drawn cells or
      a rule is drawn across the gap between them; otherwise the match stays one block, gets one
      redaction covering all its rows, and the translation flows inside it. With no drawn geometry
      at all a multi-row match is now treated as wrapped text (was: always split).
    - **Heading wrapped instead of using free space.** A chapter heading sits to the right of its
      untranslated chapter number ("第6章"), which — since FOLLOW-UP 2 made untranslated text an
      obstacle — capped the SYMMETRIC growth of the centred block, so the translation was squeezed
      into a narrow box and wrapped while the rest of the line was empty. `grow_layout_box` now takes
      the width the text needs (`metrics.text_w` at source size) and only enforces symmetry when it
      still fits; otherwise the block grows asymmetrically into the free side.
    - Tests: 4 new/updated (wrapped cell lines not split — including with no geometry at all; rows in
      different cells still split; a rule between rows keeps them split; a hemmed-in centred heading
      gets its width from the free side while symmetry still holds when the text fits).
      Full `cargo test` = **147/147**, zero warnings.
    - ⚠ VERIFY (user, Windows): re-export — no Chinese left inside table cells, and chapter headings
      on one line.
  - **FOLLOW-UP 9 (2026-08-23) — five reported defects, each measured on the parameter manual — DONE;
    NEEDS USER GUI VERIFICATION.**
    1. **Heading wrapped and drifted right.** Measured page 52: "其他配置" is 22.1 pt at x=303.5–391.8
       on a 595.4 pt page. Its centre (347.7) is 50 pt off the page centre, so it is correctly
       classified `Left` — and a Left block only grows RIGHT, giving ~235 pt for a translation that
       needs slightly more, so it wrapped. `fit_text` gained `keep_on_one_line`: a block that was ONE
       line in the source shrinks (within the existing floor) rather than breaking in two. Applied to
       single-member, non-narrow groups only — wrapping inside a table cell stays normal.
    2. **Legend-table cells left untranslated.** Measured page 2: the whole 说明/注意/警告/危险 column
       collapsed into ONE unit, because `in_same_cell` only looked at VERTICAL rules — cells stacked
       in one column share them. Only the first row's translation was ever placed, so the rest kept
       their Chinese. `collect_column_rules` → NEW `collect_rules` returning `PageRules { vertical,
       horizontal }`, and `in_same_cell` now takes glyph CENTRES and requires no horizontal rule
       between them. Re-measured: the column splits into one unit per row.
    3. **Two bottom-right cells merged into one.** Cells in the RIGHTMOST column reach the page's
       right edge, which is exactly the signal `is_wrapped_continuation` uses, so the row below was
       glued onto the row above. Since FOLLOW-UP 5 a cell's wrapped lines are already ONE unit, so
       two DIFFERENT units inside drawn cells can never be a continuation — that is now an explicit
       guard. Prose (no drawn cells) keeps the geometric rule.
    4. **Paragraph painted over the figure below it.** FOLLOW-UP 8 merged a multi-row match into one
       block whose redaction covered everything in between — including a figure. `rows_are_separated`
       now also separates rows whose vertical gap exceeds `MAX_WRAP_GAP_LINES` (1.2) line heights:
       wrapped lines follow each other immediately, anything further apart is a different place.
    5. **Value lines left in Chinese.** `shouldPassThrough` measured a letter ratio of ~0.27 for
       "取值：默认 2000，范围：0~5000，单位：°/1000。" — under the 0.3 numeric-block threshold, so these
       were skipped as "code". Chinese is dense and the real code blocks in these manuals are pure
       ASCII, so text containing two or more CJK ideographs is never passed through now.
    - Tests: 5 new (heading stays one line without passing the floor; stacked cells in one column are
      not the same cell; different units in drawn cells are never glued; rows far apart are never one
      block; CJK value lines translate while ASCII numeric blocks still pass through). Full
      `cargo test` = **151/151**, `pnpm test` = **45/45**, zero warnings, vite build ok.
    - ⚠ VERIFY (user, Windows): re-export — headings on one line, legend/table cells all translated,
      no merged cells in the rightmost column, figures not covered, and value lines in Turkish.
  - **FOLLOW-UP 10 (2026-08-23) — six remaining defects; ONE root cause covered four of them — DONE;
    NEEDS USER GUI VERIFICATION.**
    - **Rules had no extent (the big one).** `has_rule_between` and `cell_span` compared only x
      positions, so a rule anywhere on the page could act on text anywhere else. Measured on page 21:
      two path objects at **y=348–365** put edges at x≈257, which fall inside a 2.9 pt word gap of a
      sentence at **y=470–482** — and the sentence was split in two ("使用场景：选择 AMR 与 RCS" +
      "之间的通讯协议。"), each half translated and drawn separately at different sizes. Same cause on
      page 6 ("…IP 变为" + "192.168.2.1。"). `PageRules` now stores `Rule { pos, from, to }` and every
      query passes the coordinate the rule must actually reach. Re-measured: both lines are single
      units again, and the legend table still splits one unit per row.
    - **Grouping now follows drawn rules, not enclosure.** `is_wrapped_continuation` asked whether a
      box ENCLOSED each line, but these tables draw their grid as separate segments, so nothing was
      enclosed and the guard never fired — the rightmost column's cells kept merging (user items 4
      and 6) and a callout's text was glued to the line below it (item 3). It now defers to
      `rows_are_separated`, which also sees a rule drawn across the gap. That function gained the
      one-sided case: if one line sits inside a drawn box and the other does not (callout vs. the
      line beneath), they are separate by definition.
    - **Heading may reclaim the gap before it (item 1).** "第12章 校准配置": only the title run is
      replaced, so the translation grows right from x=303.5 and still did not fit. A left-aligned
      block now reclaims space on its LEFT *only when it still does not fit* — bounded by the chapter
      number — so its left edge stays put in the normal case, where it carries the page's indentation.
      With `keep_on_one_line` (FOLLOW-UP 9) the heading now fits on one line at a near-source size.
    - Tests: 3 new/updated (a rule that does not reach a height cannot bracket or split there — built
      from the measured y=348–365 vs y=476 case; a cramped left heading reclaims the gap but stops at
      the chapter number, and does not move when it already fits; one-sided containment separates).
      Full `cargo test` = **152/152**, `pnpm test` = **45/45**, zero warnings.
    - ⚠ VERIFY (user, Windows): re-export — no mid-sentence gaps/size changes in body text, callout
      text inside its box, the rightmost column's cells separate, and chapter headings on one line.
    - ⚠ VERIFY (user, Windows): (a) Config sidebar → Subtitle opens a normal opaque window (not a ghost
      square). (b) Document → translate a PDF → **Save as PDF** (layout mode) with "Translate text in images"
      ON completes; toast progresses through Binding/Analyzing/Matching/Applying. (c) Same flow with toggle
      OFF still works (byte-identical to pre-A2 A1). Agent can't see the GUI.

## 2. DO THIS NEXT (pick in this order)

### CURRENT DIRECTIVE (user spec 2026-05-30 ~12:00) — additive PDF/Audio features, then Phase 5
> This SUPERSEDES the A–F status notes below (those describe the pre-existing phases). A1, A2 **and B**
> of this directive are now **DONE** (see the §1 bullets above); **only Phase 5 remains.** Phase 5 is
> the NEXT active task — but per the directive it should start only AFTER A+B are GUI-verified by the
> user and the existing smoke tests are green, and you must PAUSE at the phase boundary for sign-off.
>
> **Binding constraints from the spec (non-negotiable / pazarlığa kapalı):** everything is ADDITIVE
> (new module / command / window label / Config sub-section / i18n key / export mode); existing Tauri
> command signatures, behaviors, return types, `invoke_handler!` entries, window labels, output modes
> and the `Document`/`Subtitle` flows stay byte-for-byte intact; new features sit BEHIND A TOGGLE so the
> default path is unchanged; the `.potext` contract, engine barrels and glossary core are CONSUMED only,
> never modified; NO `unwrap()` on user input in NEW code (existing unwraps not refactored); **every new
> crate is listed + justified BEFORE adding, and any crate whose compiled size > 10 MB needs explicit
> user approval (§18)**; every new i18n key lands in `en_US.json` + `tr_TR.json`; write tests (Rust
> in-memory + Vitest); small, single-topic commits; after each commit the existing smoke tests (Google
> "hello", existing reflow PDF export, `transcribe_media`) must still pass. **If any step requires
> changing existing structure → STOP and ASK. Default stance: add, don't change. Never commit unless
> the user explicitly says so.**

**Ground-truth deltas (the spec assumed an older/wrong layout — build on the REAL code):**
- `export_translated_pdf` ALREADY EXISTS (the reflow A4 export) → A1's in-place path shipped as
  `export_layout_pdf` (do NOT clobber the reflow command).
- Audio lives in `src-tauri/src/media.rs` — a **SINGLE FILE**, not `media/transcribe.rs`. Commands are
  `transcribe_media` / `cancel_transcription`. There is **NO ffmpeg** — transcription is the Gemini File
  API (upload→poll ACTIVE→`generateContent`). The spec's "`transcribe.rs` ffmpeg sidecar" does not exist;
  preserve `media.rs` instead. `media/live_caption.rs` can STILL be added additively via
  `pub mod live_caption;` inside `media.rs` (Rust 2018 allows `media.rs` + a `media/` submodule dir).

**Remaining, IN ORDER:**
1. **A1 — DONE** (see the §1 bullet). Only the ⚠ Windows GUI verification is left (user step).
2. **A2 — in-image (Chinese) text: OCR → translate → overlay — DONE** (see the §1 A2 bullet). Shipped as
   the isolated module `src-tauri/src/document/image_text.rs` (`scan_pdf_images` batch scan +
   legacy `extract_pdf_images` + `ImageOverlay`) + an additive `image_overlays` field on
   `export_layout_pdf`'s `LayoutRequest`, gated by the "Translate text in images" Switch (default OFF ⇒
   byte-identical to A1). OCR reuses the existing recognize stack (NO new OCR engine): the frontend reads
   each scanned PNG from `cachePath`, writes it to the cached `pot_screenshot_cut.png` for file-based
   engines (incl. Windows `system_ocr`) AND passes base64 / `convertFileSrc` for tesseract/base64 engines
   — so no recognize signature changed. Milestone 1 = TEXT-OVERLAY (white box + fitted translation over
   the image bbox); full per-glyph inpaint is the documented next step. **2026-06-18:** batch
   `scan_pdf_images` + progress events fix the per-page reload hang. Only the ⚠ Windows GUI/OCR
   verification is left (user step).
3. **B — live (real-time) captions — DONE** (see the §1 Task-B bullet). Shipped as
   `src-tauri/src/media/live_caption.rs` (`start_live_caption`/`stop_live_caption`,
   `live_caption_window`), the overlay `src/window/LiveCaption/index.jsx`, and a controller card in
   the Subtitle window. cpal 0.15.3 (user-approved) does WASAPI loopback; transcription uses INLINE
   base64 audio (not the File API); translation reuses the same Gemini path + glossary. The
   `media.rs` file-transcribe path is untouched. The spec's "new label in `tauri.conf.json`" did NOT
   apply — the real code creates ALL non-daemon windows at runtime via `window.rs` (only `daemon` is
   in `tauri.conf.json`), so the overlay follows that runtime pattern. Only the ⚠ Windows
   GUI/audio/network verification is left (user step).
4. **Phase 5 — Bookmarklet / `POST /translate_url`** (only AFTER A+B are green and smoke tests pass).
   FIRST upgrade `server.rs`'s string-match router to a real query parser (`url::Url`) — §8 warns that
   `?screenshot=false` is currently matched as a LITERAL string; ALL existing routes (`/`, `/translate`,
   `/selection_translate`, `/input_translate`, `/ocr_recognize`, `/ocr_translate`, `/config`) must keep
   working unchanged. THEN add `POST /translate_url`. Build the bookmarklet JS at build time and surface
   it in a NEW "Browser Companion" Config page (new route + page folder + sidebar entry + i18n en+tr).

Per-commit gate: new i18n (en+tr), tests for new logic, confirm existing behavior unbroken, and PAUSE
at each task boundary for user sign-off. **Still NO commits** (user is holding all commits).

### A) §9 #2 bug — glossary `scope='document'` over-filters  ✅ DONE (see §1). Skip.

### B) §7 — translation loop in a Rust background thread + Tauri progress events  ✅ DONE in code (see §1). **Now BLOCKED on user GUI/network verification** — do that first (see the ⚠ VERIFY note in §1). Only the Gemini path was moved to Rust (project is Gemini-first); other engines stay in-renderer by design.

### C) §7 — right-click context-menu integration → defer to Phase 9 (don't start now).

### D) Phase 3 (EPUB) — DONE in code + unit-tested, INCLUDING both former follow-ups (bilingual
EPUB output D(1) + cosmetic section labels D(2)) — see §1. **Now BLOCKED on user GUI/network
verification** (the ⚠ VERIFY notes in §1) — that is the only thing gating phase progress and only
the user can do it. There is NO remaining Phase-3 code work to start; do not re-implement.

### E) Phase 4 (Audio/Video → subtitle) — increment 1 + ALL FOUR follow-ups (#1 long-media
continuation, #2 large-file chunked upload, #3 non-Gemini cue translation, #4 Document→cache util)
+ the **one-click "Transcribe & Translate" UX fix** (from the user's first real video-translation
test that came back untranslated — see §1) are DONE in code + frontend-green. **There is NO remaining
Phase-4 code work.** The phase is **BLOCKED solely on user GUI/network verification** — the ⚠ VERIFY
notes in §1 (transcription, Gemini + non-Gemini cue translate, long-media continuation, >200 MB
upload, AND the new one-click flow producing a TRANSLATED export). The agent cannot do these (no GUI,
no live API). Do NOT re-implement anything. **Don't start a NEW phase (Phase 5+) until the user signs
off on the Phase 4 GUI verification AND approves the next phase** — then read the HANDOFF §1 phase
table for what's next and PAUSE at the boundary.

### F) State of the world: EVERY coded task to date (A–D + both D follow-ups + Phase 4 increment 1
+ ALL FOUR Phase-4 follow-ups + directive A1 + directive A2 + **directive B (live captions)**) is
implemented + green (`cargo test` **85/85**, `pnpm test` **22/22**, `vite build` ok). **There is no
remaining autonomous code work in the CURRENT DIRECTIVE except Phase 5** (top of §2). The pre-existing
phases + the new directive features stay gated ONLY on the user's GUI/network verification — none of
which the agent can perform — namely: the §7 Rust translate loop (the A–F "B"), Phase-3 EPUB, Phase-4
subtitles (transcription, Gemini + non-Gemini translate, long-media continuation, large upload, the
one-click flow), directive A1 (layout PDF), A2 (in-image OCR), and now directive B (live captions —
overlay shows translated text, Stop releases the device, click-through toggles). **PAUSE at the Phase-5
boundary for user sign-off before starting it.** (Note: there is no `HANDOFF.md`; this file is the full
handoff — the old "HANDOFF §1 phase table" reference is stale.) **Still NO commits** (the user is holding
all commits).

## 3. Commands
- Run app (GUI, user verifies): `pnpm tauri dev` (Rust changes => relaunch; frontend hot-reloads).
  Subtitle window opens from Config sidebar → "Audio / Video Subtitles" (`invoke('subtitle_window')`).
  Live caption: in that window's "Live Captions (system audio)" card → Start (opens the overlay via
  `invoke('live_caption_window')` + `start_live_caption`).
- Frontend tests: `pnpm test`  (vitest; currently 22/22 — incl. `src/utils/subtitle.test.js` 8/8).
- Rust tests: `cargo test` in `src-tauri` (85/85; e.g. `cargo test document::export`,
  `cargo test glossary`, `cargo test document::translate`, `cargo test document::epub` = 8/8,
  `cargo test media` = 13/13 file-transcribe + `cargo test media::live_caption` = 9/9,
  `cargo test document::pdf_layout` = 8/8, `cargo test document::image_text` = 4/4).
  NOTE: `cargo test` takes ONE substring filter — use `cargo test document::` (not two names).
  On Windows/PowerShell use `... 2>&1 | Select-Object -Last 40` (no `tail`).
- Frontend compile-check (no GUI): `pnpm exec vite build` (catches JSX errors in Document +
  Subtitle index.jsx).
- Lint: use the editor lint tool on edited files. `media.rs`, `Subtitle/index.jsx`,
  `utils/subtitle.js`, `utils/translationCache.js`, `epub.rs` + `Document/index.jsx` are clean.

## 4. Watch-outs (see HANDOFF §9 for full table)
- Free-tier Gemini RPD quota is the real wall for big docs — not a bug.
- `maxOutputTokens: 32768` hardcoded in geminipro; structured path disables thinking
  (`thinkingBudget` 0 for flash, 128 for pro) to avoid MAX_TOKENS truncation of the JSON tail.
- Glossary test entry mapping to "merhaba kanka" can leak on weak models — it's a USER DATA
  entry, advise removing it; not a code fix.
- Do NOT re-enable `tauri_plugin_single_instance` (directive #6). Do NOT start Tauri 2 (Phase 7).
- EPUB reassembly key contract: `assemble_epub` looks up translations by the SAME
  `normalize_ws` text `extract_epub` emitted (= the frontend's `para.original`). If you change
  segmentation/normalization in `epub.rs`, change BOTH `extract_segments` and the `flush_run`/
  transform path together or translations silently won't match.
- quick-xml 0.40 emits entity refs (`&amp;`, `&#169;`) as separate `Event::GeneralRef` events,
  NOT inside the text node — both EPUB walks merge text+ref runs to handle this. Keep that in mind
  if you touch the reader loops.
- Phase 4 subtitles: transcription is **Gemini-only** (audio understanding) and goes through the
  **Gemini File API** (upload → poll `ACTIVE` → `generateContent` with `file_data`). The file is
  now STREAMED from disk in 8 MiB chunks (2 GB cap, not loaded fully into RAM), and long media is
  transcribed via a CONTINUATION loop (resume-after-last-timestamp, ffmpeg-free) instead of one
  shot — so MAX_TOKENS no longer drops the tail unless the model ignores the resume hint (then it
  stops gracefully with what it had). `media.rs` shares NO state with the
  document cache/translate path except that the Subtitle window REUSES `translate_document` for
  the translate step (so the `document://*` events fire for subtitle translation too — both
  windows listen on the same channel but filter by `runId`, so they don't cross-talk in practice).
  NOTE: the subtitle TRANSLATE step now branches by the chosen `translateEngine` — only the Gemini
  branch reuses `translate_document` (events); NON-Gemini translates per-cue in the renderer (no
  events). Transcription itself is still Gemini-only.
- `derive_file_api_urls` assumes the base ends with `…/<version>/models/` (default Gemini shape);
  a heavily-rewritten proxy base could derive a wrong upload URL — if a user reports upload
  failures on a custom `requestPath`, check that function first.
- `src/utils/translationCache.js` is now the SINGLE source of the cache/RPM helpers — BOTH
  `Document/index.jsx` and `Subtitle/index.jsx` import `getEngineModelId`/`getCachedTranslation`/
  `setCachedTranslation`/`RpmLimiter` from it (Phase-4 follow-up #4 is DONE; the old duplicate
  copies in Document are gone). Change cache keying in ONE place.
- EPUB export now supports BOTH translated-only (replaces the text node) and bilingual (keeps the
  original, appends `<br/>` + `<span class="ctranslate-translation">…</span>`), selected via the
  "Save as EPUB" dropdown and the `bilingual` arg on `assemble_epub`/`transform_xhtml`/`flush_run`.
  Bilingual interleaves per text RUN, not per whole block — if a user wants strict per-block
  duplication later, that needs element-context tracking the streaming pull-parser doesn't keep.
- Task A1 layout-preserving PDF (`document/pdf_layout.rs`) edits the ORIGINAL pdf via pdfium-render
  and emits on its OWN channel `layout_export://progress|done` (does NOT collide with `document://*`).
  Commands `export_layout_pdf` / `cancel_layout_export`. `export_translated_pdf` (reflow A4) and
  `export_layout_pdf` (in-place) are TWO SEPARATE export paths — do not merge them; the "Save as PDF"
  dropdown routes `layout:` → in-place, `native:` → reflow, `print:` → webview print.
  **PDFium pin (2026-06-18):** `document/pdf.rs` MUST fetch `chromium/7543` (NOT 7843) — tag file
  `pdfium.release` in app config dir; mismatch ⇒ delete DLL + re-download. **Layout progress (2026-06-18):**
  `assemble()` emits step-by-step progress; `plan_page_edits` uses `find_run` on `text.all()` + lazy
  bbox fetch (slow `PageGeometry` fallback only on miss). Do NOT revert to full-page `loose_bounds` scan.
- A1 alignment depends on `paragraphs[].original` being a contiguous substring of pdfium's
  `page.text().all()`. If you change the frontend paragraph split in `Document/index.jsx`, the Rust
  `find_run` match degrades GRACEFULLY (unmatched blocks are skipped → original text kept, never a
  crash, but fewer blocks get translated). A1 draws with DejaVuSans (Latin/Cyrillic) — a CJK *target*
  would render missing glyphs; CJK target output is intentionally left to the reflow `export_translated_pdf`
  path (Noto). Redaction is a white box over the text bbox (best-effort; can clip a colored background
  behind text — acceptable per the brief, future toggle).
- Task A2 (in-image text) rides ON TOP of A1: it adds the `image_overlays` field to `export_layout_pdf`'s
  request and draws the overlays with A1's SAME white-box+text path. The two are NOT separate commands —
  there is ONE export pass. The DejaVuSans-glyph limitation applies (the *target* lang must be
  Latin/Cyrillic to render; a CJK target inside an image won't show — same constraint as A1, not a new
  bug). **`scan_pdf_images`** (batch, `image_scan://progress`) is the preferred extraction path;
  `extract_pdf_images` (single page) remains for compat. `ImageOverlay` is defined in `image_text.rs` and
  imported by `pdf_layout`. The OCR step REUSES the recognize services by writing each image to the
  shared cached `pot_screenshot_cut.png` (same file the screenshot→OCR flow uses) so `system_ocr` reads
  it — this momentarily overwrites that cache file, which is fine (the next screenshot overwrites it
  anyway), but be aware if you ever run image-OCR and a screenshot OCR truly concurrently. The Chinese
  gate is an OFFLINE Han-char regex (`CJK_HAN_RE`), NOT a network `detect()` call (per-image network
  detection would be slow + rate-limited). Milestone 1 whitens the WHOLE image bbox before drawing —
  great for text-only images, lossy for photos; the user toggle is OFF by default to make that opt-in.
  Next steps if asked: (a) cover ONLY the source glyphs (needs per-line OCR boxes — `system_ocr` returns
  only `.Text()`, so this needs a richer OCR path), (b) true inpaint, (c) a cancel for the OCR pre-step
  (currently the layout-export Cancel only stops the Rust drawing thread, not the JS OCR/translate loop).
- **`build_window` transparency (2026-06-18):** only `translate`, `screenshot`, `live_caption` get
  `transparent(true).decorations(false)` on Windows/Linux. Do NOT re-apply globally — `subtitle` /
  `document` windows will show as empty ghost squares behind Config.
- Task B live caption (`media/live_caption.rs`) is the ONLY thing that captures live audio. It opens the
  default OUTPUT device for INPUT (cpal WASAPI loopback) — Windows-only in practice; macOS/Linux fail
  gracefully with an `error` state. It uses INLINE base64 audio (NOT the File API `media.rs` uses) and a
  SEPARATE event channel (`live_caption` / `live_caption://state` / `live_caption://passthrough`) — it
  does NOT collide with `media://*` or `document://*`. Chunks are NON-overlapping (drain-all each
  iteration) so words can split at boundaries — acceptable best-effort; if asked to improve, add a small
  carry-over + dedup or a VAD. Translation is a per-chunk Gemini text call (no caching/dedup — live text
  is unique anyway). Click-through is window-level all-or-nothing: the overlay can ENABLE it but can't
  un-toggle itself once the mouse passes through, so the Subtitle window's Switch is authoritative
  (emits `live_caption://passthrough`). If asked for a global hotkey to toggle it, wire one via the
  existing `register_shortcut` path. The cpal callback only fires while audio plays (captions pause on
  silence — expected). cpal 0.15.3 pulls its own windows-rs version alongside the project's `windows`
  0.58 — both coexist fine (different crate versions), just adds compile time.
