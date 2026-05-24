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
    query_active_glossary(&conn, &source_lang, &target_lang, scope.as_deref())
}

/// Shared implementation for `get_active_glossary` — extracted so that
/// unit tests can call it directly with a plain `Connection` reference.
fn query_active_glossary(
    conn: &Connection,
    source_lang: &str,
    target_lang: &str,
    scope: Option<&str>,
) -> Result<Vec<GlossaryEntry>, String> {
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

// ── CRUD types ──────────────────────────────────────────────────────────────

/// Input payload for `add_glossary_entry` and `update_glossary_entry`.
/// Intentionally separate from `GlossaryEntry` — no id / timestamps.
#[derive(Debug, Deserialize)]
pub struct GlossaryEntryInput {
    pub source_term: String,
    pub target_term: String,
    pub source_lang: String,
    pub target_lang: String,
    pub scope: Option<String>,
    pub case_sensitive: bool,
    pub active: bool,
    pub notes: Option<String>,
}

/// Filter payload for `list_glossaries`.
#[derive(Debug, Default, Deserialize)]
pub struct GlossaryListFilter {
    pub source_lang: Option<String>,
    pub target_lang: Option<String>,
    pub scope: Option<String>,
    pub only_active: Option<bool>,
    pub search: Option<String>,
}

// ── Input validation ────────────────────────────────────────────────────────

/// Validate a `GlossaryEntryInput`. Returns `Ok(())` or a human-readable
/// `Err(String)` suitable for surfacing in the frontend toast.
fn validate_entry(entry: &GlossaryEntryInput) -> Result<(), String> {
    let st = entry.source_term.trim();
    if st.is_empty() {
        return Err("source_term must not be empty".into());
    }
    let tt = entry.target_term.trim();
    if tt.is_empty() {
        return Err("target_term must not be empty".into());
    }
    let sl = entry.source_lang.trim();
    if sl.is_empty() {
        return Err("source_lang must not be empty".into());
    }
    let tl = entry.target_lang.trim();
    if tl.is_empty() {
        return Err("target_lang must not be empty".into());
    }
    // Language codes are short ISO-ish tags or '*' for wildcard
    if sl.len() > 10 {
        return Err("source_lang is too long (max 10 chars)".into());
    }
    if tl.len() > 10 {
        return Err("target_lang is too long (max 10 chars)".into());
    }
    Ok(())
}

// ── CRUD commands ───────────────────────────────────────────────────────────

/// Insert a new glossary entry. Returns the auto-generated row id.
#[tauri::command]
pub fn add_glossary_entry(entry: GlossaryEntryInput) -> Result<i64, String> {
    validate_entry(&entry)?;
    let app = APP.get().ok_or("APP not initialized")?;
    let state = app
        .try_state::<GlossaryDb>()
        .ok_or("Glossary DB not initialized")?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    do_add_entry(&conn, &entry)
}

fn do_add_entry(conn: &Connection, entry: &GlossaryEntryInput) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO glossaries
            (source_term, target_term, source_lang, target_lang,
             scope, case_sensitive, active, notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            entry.source_term.trim(),
            entry.target_term.trim(),
            entry.source_lang.trim(),
            entry.target_lang.trim(),
            entry.scope.as_deref().map(str::trim),
            entry.case_sensitive,
            entry.active,
            entry.notes.as_deref().map(str::trim),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// Update an existing glossary entry by id. Also refreshes `updated_at`.
#[tauri::command]
pub fn update_glossary_entry(id: i64, entry: GlossaryEntryInput) -> Result<(), String> {
    validate_entry(&entry)?;
    let app = APP.get().ok_or("APP not initialized")?;
    let state = app
        .try_state::<GlossaryDb>()
        .ok_or("Glossary DB not initialized")?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    do_update_entry(&conn, id, &entry)
}

fn do_update_entry(conn: &Connection, id: i64, entry: &GlossaryEntryInput) -> Result<(), String> {
    let affected = conn
        .execute(
            "UPDATE glossaries SET
                source_term   = ?1,
                target_term   = ?2,
                source_lang   = ?3,
                target_lang   = ?4,
                scope         = ?5,
                case_sensitive = ?6,
                active        = ?7,
                notes         = ?8,
                updated_at    = datetime('now')
             WHERE id = ?9",
            params![
                entry.source_term.trim(),
                entry.target_term.trim(),
                entry.source_lang.trim(),
                entry.target_lang.trim(),
                entry.scope.as_deref().map(str::trim),
                entry.case_sensitive,
                entry.active,
                entry.notes.as_deref().map(str::trim),
                id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if affected == 0 {
        return Err(format!("glossary entry id={id} not found"));
    }
    Ok(())
}

/// Delete a glossary entry by id.
#[tauri::command]
pub fn delete_glossary_entry(id: i64) -> Result<(), String> {
    let app = APP.get().ok_or("APP not initialized")?;
    let state = app
        .try_state::<GlossaryDb>()
        .ok_or("Glossary DB not initialized")?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    do_delete_entry(&conn, id)
}

fn do_delete_entry(conn: &Connection, id: i64) -> Result<(), String> {
    let affected = conn
        .execute("DELETE FROM glossaries WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    if affected == 0 {
        return Err(format!("glossary entry id={id} not found"));
    }
    Ok(())
}

/// List glossary entries with optional filtering. Returns all columns
/// including inactive entries (unless `only_active` is set).
#[tauri::command]
pub fn list_glossaries(
    filter: Option<GlossaryListFilter>,
) -> Result<Vec<GlossaryEntry>, String> {
    let app = APP.get().ok_or("APP not initialized")?;
    let state = app
        .try_state::<GlossaryDb>()
        .ok_or("Glossary DB not initialized")?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    do_list_glossaries(&conn, filter.as_ref())
}

fn do_list_glossaries(
    conn: &Connection,
    filter: Option<&GlossaryListFilter>,
) -> Result<Vec<GlossaryEntry>, String> {
    // Build the query dynamically based on which filter fields are set.
    let mut sql = String::from(
        "SELECT id, source_term, target_term, source_lang, target_lang, scope,
                case_sensitive, active, notes, created_at, updated_at
         FROM glossaries WHERE 1=1",
    );
    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1u32;

    if let Some(f) = filter {
        if let Some(ref sl) = f.source_lang {
            sql.push_str(&format!(" AND source_lang = ?{param_idx}"));
            bind_values.push(Box::new(sl.clone()));
            param_idx += 1;
        }
        if let Some(ref tl) = f.target_lang {
            sql.push_str(&format!(" AND target_lang = ?{param_idx}"));
            bind_values.push(Box::new(tl.clone()));
            param_idx += 1;
        }
        if let Some(ref sc) = f.scope {
            sql.push_str(&format!(" AND scope = ?{param_idx}"));
            bind_values.push(Box::new(sc.clone()));
            param_idx += 1;
        }
        if f.only_active == Some(true) {
            sql.push_str(" AND active = 1");
        }
        if let Some(ref q) = f.search {
            let q = q.trim();
            if !q.is_empty() {
                let like = format!("%{q}%");
                sql.push_str(&format!(
                    " AND (source_term LIKE ?{pi} OR target_term LIKE ?{pi} OR notes LIKE ?{pi})",
                    pi = param_idx,
                ));
                bind_values.push(Box::new(like));
                #[allow(unused_assignments)]
                { param_idx += 1; }
            }
        }
    }

    sql.push_str(" ORDER BY datetime(updated_at) DESC");

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        bind_values.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
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

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory DB with migrations applied — no APP / Tauri
    /// needed, all tests go through the `do_*` / `query_*` helpers.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory DB");
        run_migrations(&conn).expect("migrations");
        conn
    }

    fn sample_input(src: &str, tgt: &str, sl: &str, tl: &str) -> GlossaryEntryInput {
        GlossaryEntryInput {
            source_term: src.into(),
            target_term: tgt.into(),
            source_lang: sl.into(),
            target_lang: tl.into(),
            scope: None,
            case_sensitive: false,
            active: true,
            notes: None,
        }
    }

    // ── migration ───────────────────────────────────────────────────────

    #[test]
    fn migration_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory DB");
        run_migrations(&conn).expect("first run");
        run_migrations(&conn).expect("second run");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "schema_migrations must have exactly 1 row");
    }

    // ── validation ──────────────────────────────────────────────────────

    #[test]
    fn validate_rejects_empty_source_term() {
        let e = GlossaryEntryInput {
            source_term: "   ".into(),
            ..sample_input("x", "y", "en", "tr")
        };
        assert!(validate_entry(&e).is_err());
    }

    #[test]
    fn validate_rejects_empty_target_term() {
        let e = GlossaryEntryInput {
            target_term: "".into(),
            ..sample_input("x", "y", "en", "tr")
        };
        assert!(validate_entry(&e).is_err());
    }

    #[test]
    fn validate_rejects_empty_lang() {
        let e = sample_input("hello", "merhaba", "", "tr");
        assert!(validate_entry(&e).is_err());
    }

    #[test]
    fn validate_rejects_long_lang_code() {
        let e = sample_input("hello", "merhaba", "verylonglangcode", "tr");
        assert!(validate_entry(&e).is_err());
    }

    #[test]
    fn validate_accepts_valid_input() {
        let e = sample_input("hello", "merhaba", "en", "tr");
        assert!(validate_entry(&e).is_ok());
    }

    #[test]
    fn validate_accepts_wildcard_lang() {
        let e = sample_input("hello", "merhaba", "*", "*");
        assert!(validate_entry(&e).is_ok());
    }

    // ── CRUD roundtrip ──────────────────────────────────────────────────

    #[test]
    fn add_and_list_roundtrip() {
        let conn = setup_db();
        let id = do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr"))
            .expect("add");
        assert!(id > 0);

        let all = do_list_glossaries(&conn, None).expect("list all");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].source_term, "hello");
        assert_eq!(all[0].target_term, "merhaba");
        assert_eq!(all[0].source_lang, "en");
        assert_eq!(all[0].target_lang, "tr");
        assert_eq!(all[0].id, id);
        assert!(all[0].active);
        assert!(!all[0].case_sensitive);
    }

    #[test]
    fn add_trims_whitespace() {
        let conn = setup_db();
        let e = sample_input("  hello  ", "  merhaba  ", "  en ", " tr ");
        do_add_entry(&conn, &e).expect("add");
        let all = do_list_glossaries(&conn, None).expect("list");
        assert_eq!(all[0].source_term, "hello");
        assert_eq!(all[0].target_term, "merhaba");
        assert_eq!(all[0].source_lang, "en");
        assert_eq!(all[0].target_lang, "tr");
    }

    #[test]
    fn update_entry() {
        let conn = setup_db();
        let id = do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr"))
            .expect("add");

        let updated = GlossaryEntryInput {
            source_term: "hello world".into(),
            target_term: "merhaba dünya".into(),
            source_lang: "en".into(),
            target_lang: "tr".into(),
            scope: Some("tech".into()),
            case_sensitive: true,
            active: true,
            notes: Some("test note".into()),
        };
        do_update_entry(&conn, id, &updated).expect("update");

        let all = do_list_glossaries(&conn, None).expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].source_term, "hello world");
        assert_eq!(all[0].target_term, "merhaba dünya");
        assert_eq!(all[0].scope, Some("tech".into()));
        assert!(all[0].case_sensitive);
        assert_eq!(all[0].notes, Some("test note".into()));
    }

    #[test]
    fn update_nonexistent_returns_error() {
        let conn = setup_db();
        let r = do_update_entry(&conn, 999, &sample_input("a", "b", "en", "tr"));
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("not found"));
    }

    #[test]
    fn update_refreshes_updated_at() {
        let conn = setup_db();
        let id = do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr"))
            .expect("add");
        let _before: String = conn
            .query_row(
                "SELECT updated_at FROM glossaries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();

        // SQLite datetime('now') has 1-second resolution, so we
        // artificially set the old timestamp to the past.
        conn.execute(
            "UPDATE glossaries SET updated_at = datetime('now', '-10 seconds') WHERE id = ?1",
            params![id],
        )
        .unwrap();
        let past: String = conn
            .query_row(
                "SELECT updated_at FROM glossaries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();

        do_update_entry(&conn, id, &sample_input("hello", "merhaba2", "en", "tr"))
            .expect("update");
        let after: String = conn
            .query_row(
                "SELECT updated_at FROM glossaries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();

        assert!(after > past, "updated_at must be refreshed: {after} > {past}");
    }

    #[test]
    fn delete_entry() {
        let conn = setup_db();
        let id = do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr"))
            .expect("add");

        do_delete_entry(&conn, id).expect("delete");
        let all = do_list_glossaries(&conn, None).expect("list");
        assert!(all.is_empty());
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let conn = setup_db();
        let r = do_delete_entry(&conn, 999);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("not found"));
    }

    // ── list filters ────────────────────────────────────────────────────

    #[test]
    fn list_filter_by_lang_pair() {
        let conn = setup_db();
        do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr")).unwrap();
        do_add_entry(&conn, &sample_input("bonjour", "hallo", "fr", "de")).unwrap();

        let f = GlossaryListFilter {
            source_lang: Some("en".into()),
            target_lang: Some("tr".into()),
            ..Default::default()
        };
        let result = do_list_glossaries(&conn, Some(&f)).expect("filter");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source_term, "hello");
    }

    #[test]
    fn list_filter_only_active() {
        let conn = setup_db();
        let mut inactive = sample_input("hello", "merhaba", "en", "tr");
        inactive.active = false;
        do_add_entry(&conn, &inactive).unwrap();
        do_add_entry(&conn, &sample_input("world", "dünya", "en", "tr")).unwrap();

        let f = GlossaryListFilter {
            only_active: Some(true),
            ..Default::default()
        };
        let result = do_list_glossaries(&conn, Some(&f)).expect("filter");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source_term, "world");
    }

    #[test]
    fn list_filter_search_term() {
        let conn = setup_db();
        do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr")).unwrap();
        do_add_entry(&conn, &sample_input("goodbye", "hoşçakal", "en", "tr")).unwrap();

        let f = GlossaryListFilter {
            search: Some("hell".into()),
            ..Default::default()
        };
        let result = do_list_glossaries(&conn, Some(&f)).expect("filter");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source_term, "hello");
    }

    #[test]
    fn list_filter_by_scope() {
        let conn = setup_db();
        let mut scoped = sample_input("hello", "merhaba", "en", "tr");
        scoped.scope = Some("tech".into());
        do_add_entry(&conn, &scoped).unwrap();
        do_add_entry(&conn, &sample_input("world", "dünya", "en", "tr")).unwrap();

        let f = GlossaryListFilter {
            scope: Some("tech".into()),
            ..Default::default()
        };
        let result = do_list_glossaries(&conn, Some(&f)).expect("filter");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source_term, "hello");
    }

    // ── resolution (get_active_glossary) ────────────────────────────────

    #[test]
    fn resolution_exact_beats_wildcard() {
        let conn = setup_db();
        let mut wildcard = sample_input("hello", "WILDCARD", "*", "*");
        wildcard.active = true;
        do_add_entry(&conn, &wildcard).unwrap();

        let exact = sample_input("hello", "EXACT", "en", "tr");
        do_add_entry(&conn, &exact).unwrap();

        let result = query_active_glossary(&conn, "en", "tr", None).unwrap();
        assert!(!result.is_empty());
        assert_eq!(result[0].target_term, "EXACT");
    }

    #[test]
    fn resolution_scope_beats_null_when_filter_active() {
        let conn = setup_db();
        do_add_entry(&conn, &sample_input("hello", "GLOBAL", "en", "tr")).unwrap();

        let mut scoped = sample_input("hello", "SCOPED", "en", "tr");
        scoped.scope = Some("tech".into());
        do_add_entry(&conn, &scoped).unwrap();

        let result = query_active_glossary(&conn, "en", "tr", Some("tech")).unwrap();
        assert!(!result.is_empty());
        assert_eq!(result[0].target_term, "SCOPED");
    }

    #[test]
    fn inactive_skipped() {
        let conn = setup_db();
        let mut e = sample_input("hello", "merhaba", "en", "tr");
        e.active = false;
        do_add_entry(&conn, &e).unwrap();

        let result = query_active_glossary(&conn, "en", "tr", None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn crud_full_roundtrip() {
        let conn = setup_db();

        // add
        let id1 = do_add_entry(&conn, &sample_input("hello", "merhaba", "en", "tr")).unwrap();
        let id2 = do_add_entry(&conn, &sample_input("world", "dünya", "en", "tr")).unwrap();
        assert_ne!(id1, id2);

        // list
        let all = do_list_glossaries(&conn, None).unwrap();
        assert_eq!(all.len(), 2);

        // update
        do_update_entry(&conn, id1, &sample_input("hello", "selam", "en", "tr")).unwrap();
        let all = do_list_glossaries(&conn, None).unwrap();
        let updated = all.iter().find(|e| e.id == id1).unwrap();
        assert_eq!(updated.target_term, "selam");

        // delete
        do_delete_entry(&conn, id1).unwrap();
        let all = do_list_glossaries(&conn, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id2);

        // delete remaining
        do_delete_entry(&conn, id2).unwrap();
        let all = do_list_glossaries(&conn, None).unwrap();
        assert!(all.is_empty());
    }
}
