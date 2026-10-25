use crate::error::StoreResult;
use rusqlite::{Connection, OptionalExtension};

pub const SCHEMA_VERSION: i64 = 3;
pub const FORMAT_VERSION: i64 = 3;

/// Apply the schema. Idempotent; safe to call on every open.
pub fn migrate(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
            name         TEXT PRIMARY KEY,
            indexed_at   TEXT NOT NULL,
            root_path    TEXT NOT NULL,
            generation   TEXT NOT NULL DEFAULT '',
            mode         TEXT NOT NULL DEFAULT 'full',
            file_count   INTEGER NOT NULL DEFAULT 0,
            source_bytes INTEGER NOT NULL DEFAULT 0,
            index_version INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS file_hashes (
            project  TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            rel_path TEXT NOT NULL,
            sha256   TEXT NOT NULL,
            mtime_ns INTEGER NOT NULL DEFAULT 0,
            size     INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (project, rel_path)
        );

        CREATE TABLE IF NOT EXISTS nodes (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            project        TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            label          TEXT NOT NULL,
            name           TEXT NOT NULL,
            qualified_name TEXT NOT NULL,
            file_path      TEXT NOT NULL DEFAULT '',
            start_line     INTEGER NOT NULL DEFAULT 0,
            end_line       INTEGER NOT NULL DEFAULT 0,
            properties     TEXT NOT NULL DEFAULT '{}',
            docstring      TEXT NOT NULL DEFAULT '',
            UNIQUE(project, qualified_name)
        );

        CREATE TABLE IF NOT EXISTS edges (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            project       TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            source_id     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            target_id     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            type          TEXT NOT NULL,
            properties    TEXT NOT NULL DEFAULT '{}',
            url_path_gen  TEXT GENERATED ALWAYS AS (json_extract(properties,'$.url_path')) VIRTUAL,
            local_name_gen TEXT GENERATED ALWAYS AS (
                CASE WHEN type='IMPORTS'
                     THEN coalesce(json_extract(properties,'$.local_name'),'')
                     ELSE '' END
            ) VIRTUAL,
            UNIQUE(source_id, target_id, type, local_name_gen)
        );

        CREATE TABLE IF NOT EXISTS project_summaries (
            project     TEXT PRIMARY KEY REFERENCES projects(name) ON DELETE CASCADE,
            summary     TEXT NOT NULL,
            source_hash TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS index_coverage (
            project  TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            rel_path TEXT NOT NULL,
            kind     TEXT NOT NULL,
            detail   TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (project, rel_path, kind)
        );

        CREATE TABLE IF NOT EXISTS index_coverage_meta (
            project                TEXT PRIMARY KEY REFERENCES projects(name) ON DELETE CASCADE,
            generation             TEXT NOT NULL,
            index_mode             TEXT NOT NULL,
            recorded_at            TEXT NOT NULL,
            recording_status       TEXT NOT NULL,
            ignored_files_stored   INTEGER NOT NULL DEFAULT 0,
            ignored_files_total    INTEGER NOT NULL DEFAULT 0,
            coverage_version       INTEGER NOT NULL DEFAULT 1,
            hash_records_complete  INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS observed_calls (
            project    TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            caller_qn  TEXT NOT NULL,
            callee_qn  TEXT NOT NULL,
            label      TEXT NOT NULL DEFAULT '',
            count      INTEGER NOT NULL DEFAULT 1,
            first_seen TEXT NOT NULL,
            last_seen  TEXT NOT NULL,
            PRIMARY KEY (project, caller_qn, callee_qn, label)
        );

        CREATE TABLE IF NOT EXISTS observed_paths (
            project   TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            label     TEXT NOT NULL DEFAULT '',
            path_json TEXT NOT NULL,
            count     INTEGER NOT NULL DEFAULT 1,
            first_seen TEXT NOT NULL,
            last_seen  TEXT NOT NULL,
            PRIMARY KEY (project, label, path_json)
        );

        CREATE TABLE IF NOT EXISTS node_vectors (
            node_id INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
            project TEXT NOT NULL,
            vector  BLOB NOT NULL
        );

        CREATE TABLE IF NOT EXISTS symbols (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            project    TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            qn         TEXT NOT NULL,
            name       TEXT NOT NULL,
            label      TEXT NOT NULL,
            file_path  TEXT NOT NULL,
            language   TEXT NOT NULL DEFAULT '',
            module_qn  TEXT NOT NULL DEFAULT '',
            scope      TEXT NOT NULL DEFAULT '[]',
            start_line INTEGER NOT NULL DEFAULT 0,
            is_test    INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS symbol_refs (
            project   TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            file_path TEXT NOT NULL,
            name      TEXT NOT NULL,
            kind      TEXT NOT NULL,
            PRIMARY KEY (project, file_path, name, kind)
        );

        CREATE TABLE IF NOT EXISTS file_meta (
            project       TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            rel_path      TEXT NOT NULL,
            language      TEXT NOT NULL DEFAULT '',
            module_is_dir INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (project, rel_path)
        );

        CREATE TABLE IF NOT EXISTS parse_cache (
            project  TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
            rel_path TEXT NOT NULL,
            sha256   TEXT NOT NULL,
            data     BLOB NOT NULL,
            PRIMARY KEY (project, rel_path)
        );

        CREATE TABLE IF NOT EXISTS store_meta (
            k TEXT PRIMARY KEY,
            v TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_nodes_project_label ON nodes(project, label);
        CREATE INDEX IF NOT EXISTS idx_nodes_project_name  ON nodes(project, name);
        CREATE INDEX IF NOT EXISTS idx_nodes_project_file  ON nodes(project, file_path);
        CREATE INDEX IF NOT EXISTS idx_edges_source_type   ON edges(source_id, type);
        CREATE INDEX IF NOT EXISTS idx_edges_target_type   ON edges(target_id, type);
        CREATE INDEX IF NOT EXISTS idx_edges_project_type  ON edges(project, type);
        CREATE INDEX IF NOT EXISTS idx_edges_url_path      ON edges(project, url_path_gen);
        CREATE INDEX IF NOT EXISTS idx_vectors_project     ON node_vectors(project);
        CREATE INDEX IF NOT EXISTS idx_symbols_name        ON symbols(project, name);
        CREATE INDEX IF NOT EXISTS idx_symbols_file        ON symbols(project, file_path);
        CREATE INDEX IF NOT EXISTS idx_refs_name           ON symbol_refs(project, name);
        CREATE INDEX IF NOT EXISTS idx_file_meta_lang      ON file_meta(project, language);
        "#,
    )?;

    // FTS5 contentless table with delete support. If the bundled SQLite is too
    // old for `contentless_delete`, fall back to the standard contentless form
    // (rebuild on delete).
    let fts = conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
            name_tokens, qn_tokens, label, file_path, body,
            content='', contentless_delete=1,
            tokenize='unicode61 remove_diacritics 2'
        );
        "#,
    );
    if fts.is_err() {
        conn.execute_batch(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
                name_tokens, qn_tokens, label, file_path, body,
                content='',
                tokenize='unicode61 remove_diacritics 2'
            );
            "#,
        )?;
    }

    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    conn.execute(
        "INSERT INTO store_meta(k,v) VALUES('format_version', ?1)
         ON CONFLICT(k) DO NOTHING",
        [FORMAT_VERSION.to_string()],
    )?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> StoreResult<i64> {
    Ok(conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?)
}

pub fn format_version(conn: &Connection) -> StoreResult<i64> {
    let value: Option<String> = conn
        .query_row(
            "SELECT v FROM store_meta WHERE k='format_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(FORMAT_VERSION))
}

pub fn has_fts_delete(conn: &Connection) -> bool {
    // Probe by inserting and deleting a row in a savepoint.
    let result = conn.execute_batch(
        r#"
        SAVEPOINT fts_probe;
        INSERT INTO nodes_fts(rowid,name_tokens,qn_tokens,label,file_path,body)
            VALUES(-9223372036854775808,'probe','probe','Probe','','');
        DELETE FROM nodes_fts WHERE rowid=-9223372036854775808;
        RELEASE fts_probe;
        "#,
    );
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK TO fts_probe; RELEASE fts_probe;");
        false
    } else {
        true
    }
}
