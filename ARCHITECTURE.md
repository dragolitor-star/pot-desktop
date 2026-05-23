# Architecture — CTranslate (Pot fork)

> Audit performed against upstream commit at clone time (Pot 3.0.7, Tauri 1.6.8 in README — actual dep is tauri `1.8`).
> Purpose: orient future work on Phases 1–9 (glossary, PDF/EPUB/subtitles, rebrand, distribution).

## 1. Goal of this fork
Extend Pot — a cross-platform selection/screenshot translator — with Immersive-Translate-style **document** (PDF, EPUB) and **media** (audio/video → subtitle) translation, Gemini-first, then rebrand and distribute. Do **not** rebuild what Pot already does (selection translation, screenshot OCR, 20+ engines, plugin system, HTTP API, global hotkeys, system tray, cross-platform installers).

## 2. Stack
- **Tauri 1.8** (Rust backend + WebView2/WebKit/WKWebView frontend). Tauri 2 migration deferred (Phase 7).
- **Frontend:** React 18, NextUI 2, Vite 5, JavaScript (no TS yet). NextThemesProvider + NextUIProvider in `src/main.jsx`.
- **State:** `tauri-plugin-store` (JSON key/value, app config dir). SQLite available via `tauri-plugin-sql` (already a dep) — use this for Phase 1 glossary.
- **HTTP server:** `tiny_http` on `127.0.0.1:60828` (configurable). String-match URL router, no query parsing.
- **OS-specific deps:** `windows-rs` (Windows.Media.Ocr), `macos-accessibility-client`, `window-shadows`, Linux uses `tesseract` CLI shelling out.
- **Language detection:** `lingua` crate (offline, Turkish included).
- **License:** **GPL-3.0-only** ✓ confirmed in `LICENSE` and `Cargo.toml`.

## 3. Process model — windows
Single `pot` process spawns multiple Tauri webviews. `src/App.jsx` switches on `appWindow.label` to render one of:
- **`translate`** — main translator popup (selection / input / text / image driven).
- **`screenshot`** — fullscreen overlay for region capture.
- **`recognize`** — OCR result window.
- **`config`** — settings.
- **`updater`** — updater UI.
- **`daemon`** (hidden) — used for `available_monitors()` so the Rust side can query screen geometry without a visible window. Defined in `tauri.conf.json`, instantiated lazily by `src-tauri/src/window.rs::get_daemon_window`.

Two Vite entrypoints (`index.html`, `daemon.html`) configured in `vite.config.js`.

## 4. Backend modules (`src-tauri/src/`)
| File | Responsibility |
|---|---|
| `main.rs` | Tauri builder, plugin wiring, `invoke_handler!`, system tray, single-instance, first-run logic. Holds global `APP: OnceCell<AppHandle>` and a `StringWrapper` for "text to translate" handoff. |
| `cmd.rs` | Tauri commands: `get_text`, `reload_store`, `cut_image`, `get_base64`, `copy_img`, `set_proxy`, `unset_proxy`, `install_plugin`, `run_binary` (⚠ executes `cmd /c <cmd_name>` on Windows — careful with future args), `font_list`, `open_devtools`. |
| `config.rs` | `get(key) -> Option<Value>`, `set(key, val)` over `tauri-plugin-store`. `is_first_run()`. `check_service_available()` hardcodes `builtin_*_list` registries — must edit here when adding a built-in engine. `get_plugin_list("translate"|"recognize"|"tts"|"collection")` reads `$APPCONFIG/plugins/<type>/<plugin-name>/`. |
| `server.rs` | `start_server()` spawns a thread running `tiny_http`. Router is a `match request.url() { "/" => ..., "/translate" => ..., ... }` — **no query parser** (`?screenshot=false` is matched as literal string). Always responds with text `"ok"`. Extend here for `/translate_url` in Phase 5. |
| `window.rs` | `build_window(label, title)`, mouse-monitor-aware positioning, `selection_translate / input_translate / text_translate / image_translate / ocr_recognize / ocr_translate / recognize_window / config_window / updater_window`. All emit Tauri events (`new_text`, `new_image`, `success`) to their respective webviews. |
| `hotkey.rs` | `register_shortcut("all" \| name)` + frontend-callable `register_shortcut_by_frontend(name, shortcut)`. Four hotkeys: selection/input/ocr_recognize/ocr_translate. Backed by Tauri 1's `GlobalShortcutManager`. |
| `screenshot.rs` | `screenshot(x, y)` — uses `screenshots` crate; saves PNG to `$CACHE/<bundleId>/pot_screenshot.png`. `cut_image` (in `cmd.rs`) crops to `pot_screenshot_cut.png` which all downstream consumers (OCR, image translate) read. |
| `system_ocr.rs` | Three `#[cfg]` impls of `system_ocr(lang)`: Windows uses `windows::Media::Ocr::OcrEngine`, macOS spawns bundled sidecar `resources/ocr-<arch>-apple-darwin`, Linux shells out to `tesseract`. |
| `lang_detect.rs` | `lingua`-backed detection (only inited when `translate_detect_engine = "local"`). |
| `clipboard.rs` | Clipboard monitor (start/stop), with shared `ClipboardMonitorEnableWrapper` state. |
| `tray.rs` | System tray menu + event handler (`tray_event_handler`). `update_tray` is a Tauri command — frontend can re-render menu text on language change. |
| `backup.rs` | `webdav` + `local` Tauri commands for settings backup/restore. Uses `reqwest_dav`. |
| `updater.rs` | Wraps Tauri updater check — runs at startup, points at the `pubkey` in `tauri.conf.json`. |
| `error.rs` | `thiserror` enum used across modules. |

Tauri commands registered in `main.rs::invoke_handler!`: `reload_store, get_text, cut_image, get_base64, copy_img, system_ocr, set_proxy, unset_proxy, run_binary, open_devtools, register_shortcut_by_frontend, update_tray, updater_window, screenshot, lang_detect, webdav, local, install_plugin, font_list, aliyun`.

## 5. Frontend layout (`src/`)
```
src/
├── App.jsx                  ← window-label router
├── main.jsx                 ← React root + initStore + initEnv
├── i18n/
│   ├── index.jsx            ← i18next init, fallback rules (zh_cn↔zh_tw, etc.)
│   └── locales/{lang}.json  ← 21 locales (en_US source; tr_TR currently mostly empty)
├── window/
│   ├── Translate/           ← popup translator; lists service instances as draggable cards
│   ├── Screenshot/          ← region-select overlay
│   ├── Recognize/           ← OCR result + actions
│   ├── Updater/
│   └── Config/
│       ├── index.jsx        ← sidebar + react-router shell
│       ├── routes/          ← page-to-component map
│       ├── pages/           ← {General, Translate, Recognize, Hotkey, Service, History, Backup, About}
│       └── components/SideBar
├── services/
│   ├── translate/{engine}/  ← per-engine: index.jsx, Config.jsx, info.ts
│   │   └── index.jsx        ← barrel file re-exporting every engine
│   ├── recognize/{engine}/
│   ├── tts/{engine}/
│   └── collection/{engine}/
├── utils/
│   ├── store.js             ← thin wrapper around tauri-plugin-store; watches config.json
│   ├── env.js               ← initEnv (osType, etc.)
│   ├── service_instance.ts  ← INSTANCE_NAME_CONFIG_KEY + helpers (engines instance keys look like "deepl" or "deepl@uuid")
│   └── language.ts          ← canonical language enum
├── hooks/                   ← useConfig (key, default) → [value, setValue]
└── components/WindowControl
```

## 6. Translation engine plugin contract (built-in)
Each `src/services/translate/<name>/` exports:
```js
// index.jsx
export async function translate(text, from, to, { config, setResult, detect }) { ... }
export * from './Config';   // <Config> React component for settings page
export * from './info';     // { info: { name, icon }, Language enum }
```
- Add the folder + add a line to `src/services/translate/index.jsx` (the barrel) + add the name to `builtin_translate_list` in `src-tauri/src/config.rs::check_service_available`.
- An engine **instance** key has the form `<name>` or `<name>@<uuid>` (set via `INSTANCE_NAME_CONFIG_KEY`); per-instance config lives at that key in the store.
- `stream` engines (e.g. `geminipro`) call `setResult(partial)` repeatedly; non-stream return the final string.
- Variable substitution in prompt templates: `$text`, `$from`, `$to`, `$detect`.
- `pluginList` is loaded by reading `$APPCONFIG/plugins/translate/plugin-*/info.json` at runtime — built-in vs `.potext` plugin are dispatched in `TargetArea`.

Same shape for `recognize/`, `tts/`, `collection/`.

## 7. Data flow (typical translation)
1. Trigger: global hotkey (Rust → `selection_translate`) **or** HTTP `/selection_translate` **or** UI input.
2. Rust grabs selected text via `selection::get_text()`, stores it in `StringWrapper`, opens/raises `translate` window, emits `new_text` event.
3. Frontend `Translate` window (`window/Translate/index.jsx`) reads `translate_service_list`, iterates instance keys, renders one `<TargetArea name=key>` per active engine.
4. Each `TargetArea` resolves the engine module (built-in import vs plugin shell) and calls `translate(text, from, to, { config, setResult })`.
5. Built-in engines call out via `@tauri-apps/api/http` (so traffic goes through Rust's `reqwest` with proxy honored). Plugin engines run external binaries through `run_binary` (Rust `cmd.rs`) and parse stdout JSON.

## 8. External HTTP API
`tiny_http` on `127.0.0.1:60828`, exposed for SnipDo / PopClip / `curl` / user scripts:

| Path | Behavior |
|---|---|
| `POST /` | Read body as text, call `text_translate` |
| `POST /translate` | Same as `/` |
| `GET /selection_translate` | Capture + translate current OS selection |
| `GET /input_translate` | Open empty input translation window |
| `GET /ocr_recognize` | OCR via internal screenshot |
| `GET /ocr_recognize?screenshot=false` | OCR `$CACHE/<bundleId>/pot_screenshot_cut.png` (no internal capture) |
| `GET /ocr_translate` / `?screenshot=false` | OCR + translate variant |
| `GET /config` | Open settings window |

⚠ Router string-matches the entire URL incl. query — when Phase 5 adds `/translate_url`, also upgrade to a real query parser (or `url::Url`).

## 9. OCR pipeline
Region overlay (`Screenshot` webview) → user drags rect → emits `success` → Rust callback fires `cut_image` (crops PNG) → fires `recognize_window` → `Recognize` page calls `system_ocr` or a registered recognize engine over the cached `pot_screenshot_cut.png`. macOS uses bundled Swift binary (`resources/ocr-<arch>-apple-darwin`) — **a precedent for shipping sidecar binaries** (will be the model for ffmpeg in Phase 4 and PDFium in Phase 2).

## 10. Settings storage
- File: `$APPCONFIG/com.pot-app.desktop/config.json` (e.g. `%APPDATA%\com.pot-app.desktop\config.json` on Windows).
- Plugin folder: `$APPCONFIG/com.pot-app.desktop/plugins/<type>/plugin-<name>/`.
- Cache: `$CACHE/com.pot-app.desktop/` (screenshots, cut images, future: subtitle cache).
- After rebrand (Phase 8) bundle identifier changes ⇒ all these paths move; document a migration or accept fresh start.

## 11. Plugin system (`.potext`)
ZIP archive containing `info.json` (`{ plugin_type: "translate"|"recognize"|"tts"|"collection", ... }`) + `main.js` (+ optional binary). Folder/file name **must** start with `plugin`. Installer is `cmd.rs::install_plugin` (validates name + extracts to `$APPCONFIG/plugins/<type>/`). Runtime executor is `run_binary` (spawns via `cmd /c` on Windows ⚠, direct exec elsewhere).

## 12. i18n
- 21 locales under `src/i18n/locales/`. `en_US.json` is source; `tr_TR.json` currently has only `common.ok` and empty service objects.
- Hosted on Weblate (`https://hosted.weblate.org/engage/pot-app/`) — upstream contributions go there.
- Every new key added in Phases 1–9 must land in `en_US.json` + `tr_TR.json` minimum; other locales can flow through Weblate.

## 13. Build & CI (current)
`.github/workflows/package.yml` already handles:
- macOS arm64 + x64 (`dmg` + `app.tar.gz` for updater) — signs via `APPLE_*` secrets, notarizes.
- Windows x64 + x86 + arm64 (`*-setup.exe` NSIS, plus a "fix_webview2_runtime" variant that bundles WebView2 109).
- Linux x64/x86/aarch64/armv7 (`deb` + `rpm` + AppImage for x64). Uses custom action `.github/actions/build-for-linux`.
- Updater signing via `TAURI_PRIVATE_KEY` + `TAURI_KEY_PASSWORD`.
- Post-release jobs: Homebrew cask bump, WinGet release, pot-docs trigger.

Phase 9 just needs: rebrand strings, swap pubkey/secrets, drop pot-docs trigger, optionally drop SnipDo/PopClip/`fix_webview2_runtime` build matrix.

## 14. Sidecar binary pattern (precedent for Phase 2/4)
- `tauri.macos.conf.json` adds `"resources": ["resources/*"]` to the macOS bundle.
- Sidecars copied to `target/<triple>/release/bundle/macos/<App>.app/Contents/Resources/`.
- Resolved at runtime via `app_handle.path_resolver().resolve_resource("resources/<name>")` (see `system_ocr.rs`).
- For PDFium / ffmpeg: same pattern but cross-platform — bind via `externalBin` in `tauri.conf.json` or fetch on first feature use to keep installer < 30 MB.

## 15. Extension points per phase
| Phase | Touches |
|---|---|
| **1 — Glossary** | Commit #1 (precursor): bump Gemini default model from `gemini-pro` → `gemini-2.5-flash` (add `gemini-2.5-pro` as a selectable), replace brittle SSE regex in `services/translate/geminipro/index.jsx` with a proper chunked reader (switch streaming endpoint to `?alt=sse`). Verify current model names at impl time. Then the glossary itself: `src-tauri/src/glossary.rs` (new) with SQLite tables `glossaries` **plus `schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT)` from day 1** (we don't retrofit migrations later). New Tauri command `get_active_glossary(source_lang, target_lang, scope?)`. **One shared helper** `src/utils/glossary.js` exporting `applyGlossaryToPrompt(promptList, entries)` (LLM path) and `applyGlossaryPostTranslate(text, entries)` (classical path) — every built-in LLM and classical engine calls this helper, **no copy-paste injection per engine**. New `src/window/Config/pages/Glossary/` page + route + sidebar entry. Resolution rules in `docs/glossary.md`: (a) exact src+tgt lang > wildcard, (b) domain-scoped > global, (c) most-recently-updated wins ties, (d) `active=false` skipped. **`.potext` plugins NOT modified in Phase 1** — documented as a limitation in `docs/glossary.md`; `TargetArea` gets a `// TODO(phase-X): glossary support for .potext plugins`. i18n: new `config.glossary.*` keys (en + tr). |
| **2 — PDF** | New `src-tauri/src/document/pdf.rs`. Add `pdfium-render` crate (binary sidecar). New Tauri command `translate_pdf(path, opts)` that emits progress events. New `src/window/Document/` window + route + drag-drop. Cancel via `tokio::sync::oneshot` or `AtomicBool`. |
| **3 — EPUB** | Same shape as PDF; `epub` + `zip` crates. Optional reader: extra `src/window/Reader/` with `epub.js` bundled via vite. |
| **4 — Audio → SRT** | `ffmpeg-sidecar` crate; bundle ffmpeg via `externalBin`. `src-tauri/src/media/transcribe.rs`. New Tauri commands `transcribe_file`, `cancel_transcribe`. SHA-256 cache in `$CACHE/<bundleId>/transcribe/`. New `src/window/Subtitle/` window. |
| **5 — Bookmarklet** | Add `POST /translate_url` to `server.rs` (after upgrading router to parse query params). Bookmarklet JS generated at build time, surfaced in a new "Browser Companion" Config page. |
| **6 — TS migration** | `tsconfig.json` with `allowJs: true`; convert new files first (Phase 1+ landed in TS where possible). |
| **7 — Tauri 2** | Deferred. Plugin imports + capability/permission model change. Estimate 2+ weeks. |
| **8 — Rebrand** | `package.json` name, `Cargo.toml` package.name, `tauri.conf.json` (productName, bundle.identifier, updater.endpoints, updater.pubkey), `src-tauri/icons/*`, `src-tauri/icons_mac/*`, all README* files, every `<title>Pot</title>` in HTML, copyright in `bundle.copyright`, daemon window title in `tauri.conf.json`, system tray identifiers, About page text in i18n. **Fresh start — no settings migration from `com.pot-app.desktop` to the new identifier** (per user decision). Document in `distribution/README-for-users.{en,tr}.md` so existing Pot users know previous settings won't carry over. |
| **9 — Distribution** | Replace pot-app secrets, regen Tauri updater keypair, host `update.json` on the new GitHub Releases. Drop trademark-bound steps (Homebrew tap `pot-app/homebrew-tap`, Winget `Pylogmon.pot`). Add `SECURITY.md`, fresh `CHANGELOG.md`, `distribution/README-for-users.{en,tr}.md`. Code-sign Windows via `WINDOWS_CERT_PFX_BASE64`/`WINDOWS_CERT_PASSWORD`. |

## 16. Quirks, debts, and watch-outs
1. **Gemini engine is outdated.** Default `requestPath` is `https://generativelanguage.googleapis.com/v1beta/models/gemini-pro` — `gemini-pro` is a deprecated model name. Per project brief: use `gemini-2.5-flash` for translation, `gemini-2.5-pro` for long-context. Update default in `src/services/translate/geminipro/Config.jsx:23` and verify model name is current at implementation time. The streaming SSE parser uses brittle regex `/{ \"text\": \".*\" } ],/` — consider replacing with proper SSE chunk parser when touched.
2. **Identifier collision risk.** `bundle.identifier = com.pot-app.desktop` — must change in Phase 8 to avoid clashing with installed Pot.
3. **HTTP server is permissive.** Binds only to `127.0.0.1` but **no auth** — any local process can trigger translation/screenshot. Acceptable for desktop tool; document in `SECURITY.md`.
4. **`run_binary` security.** `Command::new("cmd").args(["/c", &cmd_name])` on Windows — `cmd_name` comes from plugin config. Plugin trust model is "you installed it, you trust it." Note in SECURITY.md.
5. **`unwrap()` density** in `window.rs`, `screenshot.rs`, `cmd.rs` — Rust style rule (no `unwrap()` on user input) applies to **new** code; refactoring existing unwraps is out of scope.
6. **pnpm 11 `ERR_PNPM_IGNORED_BUILDS`** for `esbuild` and `tesseract.js` — postinstalls are gated. esbuild ships platform binary as optionalDependency so vite still works; tesseract.js postinstall is cache-related. If a developer needs the postinstalls, run `pnpm approve-builds` once.
7. **Typo:** `server.rs:20` notification ID is `"com.pot-spp.com"` (should be `"com.pot-app.com"`). Bug, but isolated to one notification.
8. **Daemon window** is a workaround for `available_monitors()` needing a window handle — keep it through Tauri 2 migration unless the new API lifts that constraint.
9. **`tauri_plugin_single_instance` disabled — decision: keep disabled for release.** The init call at `src-tauri/src/main.rs:46-53` is commented out. **Root cause:** `plugins-workspace` v1 branch (commit `fa8ee1d`) introduced a Windows null-pointer dereference under newer Rust UB checks; single-instance is a nice-to-have (prevents double-launch), not worth the crash risk for end users. The comment-out + a `// TODO` referencing the upstream fix commit are now committed (no longer a "dev tweak"). Revisit when (a) plugins-workspace v1 ships a fix, or (b) we hit Tauri 2 in Phase 7 (this plugin is rewritten there, making the issue moot).

## 17. Phase 0 verification status

**Working directory:** `C:\Users\enesk\pot-desktop` (NOT `C:\CTranslate`). Origin = `dragolitor-star/pot-desktop` (user's fork). Upstream = `pot-app/pot-desktop`. As of clone time: 0 commits diverged.

| Check | Status |
|---|---|
| GPLv3 license | ✓ confirmed (`LICENSE` head; `Cargo.toml` license field). |
| README mapped | ✓ EN read; ZH/KR are translations. |
| Backend modules mapped | ✓ all 14 files in `src-tauri/src/` covered. |
| Frontend layout mapped | ✓ window/services/i18n/utils. |
| Plugin contract documented | ✓ (§6). |
| HTTP API documented | ✓ (§8). |
| `pnpm install` | ✓ `node_modules` already present in pot-desktop. (Fresh install verified on C:\CTranslate — same 548 packages, same `ERR_PNPM_IGNORED_BUILDS` warning per §16.6.) |
| `pnpm tauri dev` smoke test | ⏸ Pending — will run in `C:\Users\enesk\pot-desktop` (already built; `target/debug` exists). Visual verification (translate "hello" via Google engine) is a human step. Automation can confirm: vite dev server up, Rust compile clean, app process alive ≥ 10 s, `logs/pot.log` panic-free. |
| `cargo check` | ✓ Implicit — `target/debug` exists, so prior `tauri dev` has compiled. |

## 18. What changes in Phase 1+
- **New top-level Rust module** per major feature (`glossary.rs`, `document/`, `media/`) registered in `main.rs`.
- **New Tauri commands** added to `invoke_handler!` macro.
- **New Config pages** = new route in `src/window/Config/routes/index.jsx` + new page folder in `pages/` + new sidebar entry in `components/SideBar`.
- **New i18n keys** land in `en_US.json` (source) + `tr_TR.json` (Turkish, our default fork audience).
- **New crates** > 10 MB compiled size require explicit approval per project rules.
