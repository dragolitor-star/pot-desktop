// Translation cache — Phase 2 optimization.
//
// Backed by a dedicated SQLite database at `$APPCONFIG/<bundleId>/translation_cache.db`.
// Same `rusqlite` driver used by the glossary module — kept in a separate DB file so
// glossary CRUD scans and cache lookups don't share locks. The `schema_migrations`
// table is replicated here (each store is independently versioned).
//
// Cache key contract (computed on the frontend side, before invoke):
//   sha256(model_id + "\n" + src_lang + "\n" + tgt_lang + "\n" + source_text)
//
// where `model_id` is a stable per-engine string (e.g. "gemini:gemini-3.5-flash",
// "openai:gpt-4o", "google"). Changing the model invalidates the cache entry — the
// previous translation stays in the table but is no longer keyed by current params.

use crate::error::Error;
use crate::APP;
use log::{info, warn};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::sync::Mutex;
use tauri::Manager;

const CREATE_MIGRATIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        applied_at TEXT NOT NULL
    );
";

const SCHEMA_V1: &str = "
    CREATE TABLE IF NOT EXISTS translation_cache (
        hash TEXT PRIMARY KEY,
        model TEXT NOT NULL,
        src_lang TEXT NOT NULL,
        tgt_lang TEXT NOT NULL,
        source_text TEXT NOT NULL,
        translated_text TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        last_used_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_cache_lang
        ON translation_cache(src_lang, tgt_lang);
    CREATE INDEX IF NOT EXISTS idx_cache_last_used
        ON translation_cache(last_used_at);
";

pub struct CacheDb(pub Mutex<Connection>);

/// Open (or create) the translation cache DB and run any pending migrations.
/// Fail-soft: warns and continues — cache misses just degrade to live API calls.
pub fn init_cache_db(app: &mut tauri::App) {
    match try_init(app) {
        Ok(()) => info!("Translation cache initialized"),
        Err(e) => warn!(
            "Translation cache init failed: {e} — translations won't be reused across runs"
        ),
    }
}

fn try_init(app: &mut tauri::App) -> Result<(), Error> {
    let config_dir = dirs::config_dir().ok_or_else(|| {
        let boxed: Box<dyn std::error::Error> = "user config dir not found".into();
        Error::Error(boxed)
    })?;
    let app_dir = config_dir.join(app.config().tauri.bundle.identifier.clone());
    std::fs::create_dir_all(&app_dir)?;
    let db_path = app_dir.join("translation_cache.db");
    info!("Translation cache DB path: {:?}", db_path);

    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    run_migrations(&conn)?;
    app.manage(CacheDb(Mutex::new(conn)));
    Ok(())
}

fn run_migrations(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(CREATE_MIGRATIONS_TABLE)?;
    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if current < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, datetime('now'))",
            params![1_i64],
        )?;
        info!("Translation cache schema migrated to v1");
    }
    Ok(())
}

/// Look up a cached translation by hash. Returns None on miss or any error.
/// Bumps `last_used_at` on hit so an LRU pruner can later free cold entries.
#[tauri::command]
pub fn cache_get_translation(hash: String) -> Result<Option<String>, String> {
    let app = APP.get().ok_or_else(|| "APP not initialized".to_string())?;
    let state = app
        .try_state::<CacheDb>()
        .ok_or_else(|| "Translation cache not initialized".to_string())?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let result: Option<String> = conn
        .query_row(
            "SELECT translated_text FROM translation_cache WHERE hash = ?1",
            params![hash],
            |row| row.get(0),
        )
        .ok();

    if result.is_some() {
        // best-effort LRU touch — never fail the read because of this
        let _ = conn.execute(
            "UPDATE translation_cache SET last_used_at = datetime('now') WHERE hash = ?1",
            params![hash],
        );
    }

    Ok(result)
}

/// Insert or replace a cache entry. `INSERT OR REPLACE` keeps the row count
/// stable for re-runs that update the translation text (e.g. user fixed a typo
/// in their glossary and re-translates).
#[tauri::command]
pub fn cache_set_translation(
    hash: String,
    model: String,
    src_lang: String,
    tgt_lang: String,
    source_text: String,
    translated_text: String,
) -> Result<(), String> {
    let app = APP.get().ok_or_else(|| "APP not initialized".to_string())?;
    let state = app
        .try_state::<CacheDb>()
        .ok_or_else(|| "Translation cache not initialized".to_string())?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO translation_cache
            (hash, model, src_lang, tgt_lang, source_text, translated_text,
             created_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), datetime('now'))",
        params![hash, model, src_lang, tgt_lang, source_text, translated_text],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[derive(Debug, Serialize)]
pub struct CacheStats {
    pub entry_count: i64,
    pub total_source_chars: i64,
    pub total_translated_chars: i64,
}

#[tauri::command]
pub fn cache_stats() -> Result<CacheStats, String> {
    let app = APP.get().ok_or_else(|| "APP not initialized".to_string())?;
    let state = app
        .try_state::<CacheDb>()
        .ok_or_else(|| "Translation cache not initialized".to_string())?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let (count, src_chars, tgt_chars): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(LENGTH(source_text)), 0),
                    COALESCE(SUM(LENGTH(translated_text)), 0)
             FROM translation_cache",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;

    Ok(CacheStats {
        entry_count: count,
        total_source_chars: src_chars,
        total_translated_chars: tgt_chars,
    })
}

/// Clear cache entries. With `older_than_days` set, only entries whose
/// `last_used_at` is older than the threshold are removed. Without it, the
/// whole table is wiped. Returns the number of rows deleted.
#[tauri::command]
pub fn cache_clear(older_than_days: Option<i64>) -> Result<i64, String> {
    let app = APP.get().ok_or_else(|| "APP not initialized".to_string())?;
    let state = app
        .try_state::<CacheDb>()
        .ok_or_else(|| "Translation cache not initialized".to_string())?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let affected = match older_than_days {
        Some(days) => conn
            .execute(
                "DELETE FROM translation_cache
                 WHERE last_used_at < datetime('now', '-' || ?1 || ' days')",
                params![days],
            )
            .map_err(|e| e.to_string())?,
        None => conn
            .execute("DELETE FROM translation_cache", [])
            .map_err(|e| e.to_string())?,
    };

    Ok(affected as i64)
}
