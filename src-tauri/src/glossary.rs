// Glossary storage for the translation pipeline.
//
// Backed by a dedicated SQLite database at `$APPCONFIG/<bundleId>/glossary.db`.
// We use rusqlite (synchronous, small) instead of routing through tauri-plugin-sql
// because the lookup happens on every translation request and we want the path to
// be a direct in-process call rather than an IPC round-trip through the plugin.
//
// Schema versioning lives in `schema_migrations` from day 1 so Phase 2+ (PDF document
// scopes, EPUB tags, subtitle vocabularies) can extend the schema without retrofitting
// migration plumbing.
//
// Resolution rules for `get_active_glossary` (also documented in docs/glossary.md):
//   (a) exact source+target lang match wins over wildcard ("*")
//   (b) domain-scoped match wins over scope=NULL when a scope filter is active
//   (c) ties broken by most-recently-updated
//   (d) `active = false` rows are skipped entirely

use crate::error::Error;
use crate::APP;
use log::{info, warn};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::Manager;

const CREATE_MIGRATIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        applied_at TEXT NOT NULL
    );
";

// v1 — initial glossary table + a single covering index for the hot lookup path.
const SCHEMA_V1: &str = "
    CREATE TABLE IF NOT EXISTS glossaries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        source_term TEXT NOT NULL,
        target_term TEXT NOT NULL,
        source_lang TEXT NOT NULL,
        target_lang TEXT NOT NULL,
        scope TEXT,
        case_sensitive INTEGER NOT NULL DEFAULT 0,
        active INTEGER NOT NULL DEFAULT 1,
        notes TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_glossaries_active_lang
        ON glossaries(active, source_lang, target_lang);
";

pub struct GlossaryDb(pub Mutex<Connection>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub id: i64,
    pub source_term: String,
    pub target_term: String,
    pub source_lang: String,
    pub target_lang: String,
    pub scope: Option<String>,
    pub case_sensitive: bool,
    pub active: bool,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Open (or create) the glossary DB and run any pending migrations.
/// Logs failure and continues — glossary lookups will fail-soft if the DB
/// is unavailable, so the rest of the app stays usable.
pub fn init_glossary_db(app: &mut tauri::App) {
    match try_init(app) {
        Ok(()) => info!("Glossary DB initialized"),
        Err(e) => warn!("Glossary DB init failed: {e} — glossary features will be disabled"),
    }
}

fn try_init(app: &mut tauri::App) -> Result<(), Error> {
    let config_dir = dirs::config_dir().ok_or_else(|| {
        let boxed: Box<dyn std::error::Error> = "user config dir not found".into();
        Error::Error(boxed)
    })?;
    let app_dir = config_dir.join(app.config().tauri.bundle.identifier.clone());
    std::fs::create_dir_all(&app_dir)?;
    let db_path = app_dir.join("glossary.db");
    info!("Glossary DB path: {:?}", db_path);

    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    run_migrations(&conn)?;
    app.manage(GlossaryDb(Mutex::new(conn)));
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
    info!("Glossary schema current version: {current}");

    if current < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, datetime('now'))",
            params![1_i64],
        )?;
        info!("Glossary schema migrated to v1");
    }
    Ok(())
}

/// Look up all active glossary entries that apply to a given language pair (and
/// optional domain scope). Returned in priority order: exact lang match first,
/// then scoped-vs-global tie-breaker, then most-recently-updated.
///
/// Callers (frontend translate dispatcher) typically call this once per
/// translation and pass the result to `applyGlossaryToPrompt`
/// (LLM engines) or `applyGlossaryPostTranslate` (classical engines).
#[tauri::command]
pub fn get_active_glossary(
    source_lang: String,
    target_lang: String,
    scope: Option<String>,
) -> Result<Vec<GlossaryEntry>, String> {
    let app = APP.get().ok_or_else(|| "APP not initialized".to_string())?;
    let state = app
        .try_state::<GlossaryDb>()
        .ok_or_else(|| "Glossary DB not initialized".to_string())?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let sql = "
        SELECT id, source_term, target_term, source_lang, target_lang, scope,
               case_sensitive, active, notes, created_at, updated_at
        FROM glossaries
        WHERE active = 1
          AND (source_lang = ?1 OR source_lang = '*')
          AND (target_lang = ?2 OR target_lang = '*')
          AND (?3 IS NULL OR scope IS NULL OR scope = ?3)
        ORDER BY
          (CASE WHEN source_lang = ?1 AND target_lang = ?2 THEN 0 ELSE 1 END),
          (CASE WHEN ?3 IS NOT NULL AND scope = ?3 THEN 0 ELSE 1 END),
          datetime(updated_at) DESC
    ";

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![source_lang, target_lang, scope], |row| {
            Ok(GlossaryEntry {
                id: row.get(0)?,
                source_term: row.get(1)?,
                target_term: row.get(2)?,
                source_lang: row.get(3)?,
                target_lang: row.get(4)?,
                scope: row.get(5)?,
                case_sensitive: row.get::<_, bool>(6)?,
                active: row.get::<_, bool>(7)?,
                notes: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|e| e.to_string())?);
    }
    Ok(entries)
}
