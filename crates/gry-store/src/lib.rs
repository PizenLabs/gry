//! SQLite persistence for the gry knowledge graph.
//!
//! One database holds all projects. Publication of a new generation is a single
//! transaction: readers see the previous committed snapshot until the new graph
//! is fully written and validated. A failed run therefore leaves the previous
//! index intact without a staging database.

pub mod artifact;
pub mod error;
pub mod schema;
pub mod search;
#[doc(hidden)]
pub mod test_support;
pub mod traverse;

use crate::error::{StoreError, StoreResult};
use gry_core::config::Config;
use gry_core::tokenize::{split_identifier, split_text};
use gry_core::types::{labels, Edge, FileHash, NewNode, Node, Project};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use schema::{FORMAT_VERSION, SCHEMA_VERSION};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub name: String,
    pub indexed_at: String,
    pub root_path: String,
    pub generation: String,
    pub mode: String,
    pub file_count: i64,
    pub source_bytes: i64,
    pub index_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageRow {
    pub rel_path: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMeta {
    pub generation: String,
    pub index_mode: String,
    pub recorded_at: String,
    pub recording_status: String,
    pub ignored_files_stored: i64,
    pub ignored_files_total: i64,
    pub coverage_version: i64,
    pub hash_records_complete: bool,
}

#[derive(Debug, Clone)]
pub struct EdgeSpec {
    pub source_qn: String,
    pub target_qn: String,
    pub edge_type: String,
    pub properties: Value,
}

impl EdgeSpec {
    pub fn new(
        source_qn: impl Into<String>,
        target_qn: impl Into<String>,
        edge_type: impl Into<String>,
    ) -> Self {
        Self {
            source_qn: source_qn.into(),
            target_qn: target_qn.into(),
            edge_type: edge_type.into(),
            properties: Value::Object(Map::new()),
        }
    }

    pub fn property(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        if let Value::Object(ref mut map) = self.properties {
            map.insert(key.into(), value.into());
        }
        self
    }
}

/// Registry symbol row used by incremental resolution.
#[derive(Debug, Clone)]
pub struct SymbolRow {
    pub qn: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub language: String,
    pub module_qn: String,
    pub scope: Vec<String>,
    pub start_line: i32,
    pub is_test: bool,
}

/// Extraction reference used to find files affected by a symbol change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefRow {
    pub file_path: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct FileMetaRow {
    pub rel_path: String,
    pub language: String,
    pub module_is_dir: bool,
}

/// Everything needed to atomically publish one project generation.
pub struct ProjectWrite<'a> {
    pub project: &'a str,
    pub root_path: &'a str,
    pub mode: &'a str,
    pub generation: &'a str,
    pub files: &'a [FileHash],
    pub nodes: &'a [NewNode],
    pub edges: &'a [EdgeSpec],
    pub vectors: &'a [(String, Vec<i8>)],
    pub parse_cache: &'a [(String, String, Vec<u8>)],
    pub coverage: &'a [CoverageRow],
    pub coverage_meta: Option<&'a CoverageMeta>,
    pub replace_nodes: bool,
    pub symbols: &'a [SymbolRow],
    pub refs: &'a [RefRow],
    pub file_meta: &'a [FileMetaRow],
    /// Delta: files whose outgoing edges and nodes were recomputed.
    pub affected_files: &'a [String],
    /// Delta: files removed from the project.
    pub deleted_files: &'a [String],
    /// Delta: qualified names removed by this update (nodes cascade edges).
    pub removed_qns: &'a [String],
    /// Delta: run global orphan collection (env/channel/process nodes).
    pub gc_globals: bool,
    /// Delta: only node properties changed; keep every existing edge.
    pub nodes_only: bool,
}

impl<'a> ProjectWrite<'a> {
    pub fn new(project: &'a str, root_path: &'a str, mode: &'a str, generation: &'a str) -> Self {
        Self {
            project,
            root_path,
            mode,
            generation,
            files: &[],
            nodes: &[],
            edges: &[],
            vectors: &[],
            parse_cache: &[],
            coverage: &[],
            coverage_meta: None,
            replace_nodes: true,
            symbols: &[],
            refs: &[],
            file_meta: &[],
            affected_files: &[],
            deleted_files: &[],
            removed_qns: &[],
            gc_globals: false,
            nodes_only: false,
        }
    }
}

pub struct Store {
    pub(crate) conn: Connection,
    pub path: PathBuf,
    pub(crate) fts_delete: bool,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        configure(&conn)?;
        schema::migrate(&conn)?;
        let fts_delete = schema::has_fts_delete(&conn);
        Ok(Self {
            conn,
            path,
            fts_delete,
        })
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        configure(&conn)?;
        schema::migrate(&conn)?;
        let fts_delete = schema::has_fts_delete(&conn);
        Ok(Self {
            conn,
            path: PathBuf::from(":memory:"),
            fts_delete,
        })
    }

    pub fn open_config(config: &Config) -> StoreResult<Self> {
        Self::open(&config.db_path)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> StoreResult<i64> {
        schema::schema_version(&self.conn)
    }

    pub fn format_version(&self) -> StoreResult<i64> {
        schema::format_version(&self.conn)
    }

    pub fn db_size(&self) -> i64 {
        std::fs::metadata(&self.path)
            .map(|m| m.len() as i64)
            .unwrap_or(0)
    }

    // ── Projects ──────────────────────────────────────────────────────

    pub fn upsert_project(&self, project: &str, root_path: &str) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO projects(name,indexed_at,root_path) VALUES(?1,?2,?3)
             ON CONFLICT(name) DO UPDATE SET indexed_at=excluded.indexed_at,
                                            root_path=excluded.root_path",
            params![project, now_iso(), root_path],
        )?;
        Ok(())
    }

    pub fn project(&self, name: &str) -> StoreResult<Option<ProjectRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT name,indexed_at,root_path,generation,mode,file_count,source_bytes,index_version
                 FROM projects WHERE name=?1",
                [name],
                |row| {
                    Ok(ProjectRecord {
                        name: row.get(0)?,
                        indexed_at: row.get(1)?,
                        root_path: row.get(2)?,
                        generation: row.get(3)?,
                        mode: row.get(4)?,
                        file_count: row.get(5)?,
                        source_bytes: row.get(6)?,
                        index_version: row.get(7)?,
                        nodes: None,
                        edges: None,
                        database_size: None,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn require_project(&self, name: &str) -> StoreResult<ProjectRecord> {
        self.project(name)?
            .ok_or_else(|| StoreError::not_found(name))
    }

    pub fn project_by_root(&self, root_path: &str) -> StoreResult<Option<ProjectRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT name FROM projects WHERE root_path=?1 ORDER BY indexed_at DESC LIMIT 1",
                [root_path],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match row {
            Some(name) => self.project(&name),
            None => Ok(None),
        }
    }

    pub fn list_projects(
        &self,
        limit: usize,
        offset: usize,
        stats: bool,
    ) -> StoreResult<(Vec<ProjectRecord>, i64)> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;
        let mut stmt = self.conn.prepare(
            "SELECT name,indexed_at,root_path,generation,mode,file_count,source_bytes,index_version
             FROM projects ORDER BY name LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            Ok(ProjectRecord {
                name: row.get(0)?,
                indexed_at: row.get(1)?,
                root_path: row.get(2)?,
                generation: row.get(3)?,
                mode: row.get(4)?,
                file_count: row.get(5)?,
                source_bytes: row.get(6)?,
                index_version: row.get(7)?,
                nodes: None,
                edges: None,
                database_size: None,
            })
        })?;
        let mut projects = rows.collect::<Result<Vec<_>, _>>()?;
        if stats {
            for project in &mut projects {
                let (nodes, edges) = self.counts(&project.name)?;
                project.nodes = Some(nodes);
                project.edges = Some(edges);
                project.database_size = Some(self.db_size());
            }
        }
        Ok((projects, total))
    }

    pub fn projects(&self) -> StoreResult<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name,indexed_at,root_path FROM projects ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                name: row.get(0)?,
                indexed_at: row.get(1)?,
                root_path: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn delete_project(&mut self, project: &str) -> StoreResult<bool> {
        let exists: bool = self
            .conn
            .query_row("SELECT 1 FROM projects WHERE name=?1", [project], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        let tx = self.conn.transaction()?;
        clear_fts_project(&tx, project, self.fts_delete)?;
        tx.execute("DELETE FROM projects WHERE name=?1", [project])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn counts(&self, project: &str) -> StoreResult<(i64, i64)> {
        let nodes: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE project=?1",
            [project],
            |row| row.get(0),
        )?;
        let edges: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE project=?1",
            [project],
            |row| row.get(0),
        )?;
        Ok((nodes, edges))
    }

    /// Remove nodes whose file is no longer in the project's current file set.
    /// Returns the number of orphan file paths cleaned.
    pub fn prune_orphan_nodes(&mut self, project: &str) -> StoreResult<usize> {
        let tx = self.conn.transaction()?;
        let pruned = delete_orphan_nodes(&tx, project)?;
        // Global nodes (EnvVar, Package, Process, ...) have an empty
        // file_path, so the file-level reconcile cannot see them. Remove the
        // ones left with no edge at all.
        let orphan_globals = "project=?1 AND label IN
             ('Package','EnvVar','Channel','Process','Image','Resource','Chart')
             AND id NOT IN (SELECT source_id FROM edges WHERE project=?1)
             AND id NOT IN (SELECT target_id FROM edges WHERE project=?1)";
        if self.fts_delete {
            let sql = format!("DELETE FROM nodes_fts WHERE rowid IN (SELECT id FROM nodes WHERE {orphan_globals})");
            tx.execute(&sql, [project])?;
        }
        tx.execute(
            &format!("DELETE FROM nodes WHERE {orphan_globals}"),
            [project],
        )?;
        tx.commit()?;
        Ok(pruned)
    }

    pub fn label_counts(&self, project: &str) -> StoreResult<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT label, COUNT(*) FROM nodes WHERE project=?1
             GROUP BY label ORDER BY COUNT(*) DESC, label",
        )?;
        let rows = stmt.query_map([project], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn edge_type_counts(&self, project: &str) -> StoreResult<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT type, COUNT(*) FROM edges WHERE project=?1
             GROUP BY type ORDER BY COUNT(*) DESC, type",
        )?;
        let rows = stmt.query_map([project], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ── File hashes ───────────────────────────────────────────────────

    pub fn file_hashes(&self, project: &str) -> StoreResult<Vec<FileHash>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path,sha256,mtime_ns,size FROM file_hashes
             WHERE project=?1 ORDER BY rel_path",
        )?;
        let rows = stmt.query_map([project], |row| {
            Ok(FileHash {
                rel_path: row.get(0)?,
                sha256: row.get(1)?,
                mtime_ns: row.get(2)?,
                size: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ── Publication ───────────────────────────────────────────────────

    /// Atomically replace a project's graph with the supplied generation.
    pub fn publish(&mut self, write: &ProjectWrite<'_>) -> StoreResult<PublishStats> {
        let tx = self.conn.transaction()?;
        let now = now_iso();
        let mut delta_touched: HashSet<i64> = HashSet::new();
        let timing = std::env::var_os("GRY_TIMING").is_some();
        let mut phase_t = std::time::Instant::now();
        macro_rules! phase {
            ($label:expr) => {
                if timing {
                    eprintln!(
                        "[gry]   publish {:<18} {:>6} ms",
                        $label,
                        phase_t.elapsed().as_millis()
                    );
                    phase_t = std::time::Instant::now();
                }
            };
        }

        if write.replace_nodes {
            clear_fts_project(&tx, write.project, self.fts_delete)?;
            tx.execute("DELETE FROM nodes WHERE project=?1", [write.project])?;
            tx.execute("DELETE FROM node_vectors WHERE project=?1", [write.project])?;
            tx.execute("DELETE FROM file_hashes WHERE project=?1", [write.project])?;
            tx.execute("DELETE FROM parse_cache WHERE project=?1", [write.project])?;
            tx.execute(
                "DELETE FROM index_coverage WHERE project=?1",
                [write.project],
            )?;
            tx.execute(
                "DELETE FROM index_coverage_meta WHERE project=?1",
                [write.project],
            )?;
            tx.execute("DELETE FROM symbols WHERE project=?1", [write.project])?;
            tx.execute("DELETE FROM symbol_refs WHERE project=?1", [write.project])?;
            tx.execute("DELETE FROM file_meta WHERE project=?1", [write.project])?;
        } else {
            // Delta: capture the nodes whose outgoing edges disappear, so
            // importance can be recomputed for the surviving neighbors.
            if !write.nodes_only {
                collect_delta_touched(&tx, write, &mut delta_touched)?;
                delete_outgoing_for_files(&tx, write.project, write.affected_files)?;
                delete_qns(&tx, write.project, write.removed_qns)?;
                let mut stale: Vec<&str> =
                    write.affected_files.iter().map(String::as_str).collect();
                stale.extend(write.deleted_files.iter().map(String::as_str));
                delete_file_rows(&tx, write.project, &stale)?;
            }
        }

        phase!("pre-delete");
        tx.execute(
            "INSERT INTO projects(name,indexed_at,root_path,generation,mode,file_count,source_bytes,index_version)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(name) DO UPDATE SET indexed_at=excluded.indexed_at,
                 root_path=excluded.root_path, generation=excluded.generation,
                 mode=excluded.mode, file_count=excluded.file_count,
                 source_bytes=excluded.source_bytes, index_version=excluded.index_version",
            params![
                write.project,
                now,
                write.root_path,
                write.generation,
                write.mode,
                write.files.len() as i64,
                write.files.iter().map(|f| f.size).sum::<i64>(),
                FORMAT_VERSION,
            ],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO file_hashes(project,rel_path,sha256,mtime_ns,size)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(project,rel_path) DO UPDATE SET sha256=excluded.sha256,
                    mtime_ns=excluded.mtime_ns, size=excluded.size",
            )?;
            for file in write.files {
                stmt.execute(params![
                    write.project,
                    file.rel_path,
                    file.sha256,
                    file.mtime_ns,
                    file.size
                ])?;
            }
        }

        if !write.replace_nodes {
            // file_hashes now describes the current file set. Any node whose
            // file is no longer present is stale: the file was deleted, newly
            // ignored, or left behind by an older generation. Reconcile before
            // inserting the new nodes so a full re-index actually shrinks.
            delete_orphan_nodes(&tx, write.project)?;
        }
        if !write.replace_nodes && self.fts_delete {
            delete_fts_for_files(&tx, write.project, write.affected_files)?;
        }
        phase!("project+files");
        let mut qn_to_id: HashMap<String, i64> = HashMap::with_capacity(write.nodes.len() * 2);
        let mut inserted_nodes = 0i64;
        {
            let mut node_stmt = tx.prepare(
                "INSERT INTO nodes(project,label,name,qualified_name,file_path,start_line,end_line,properties,docstring)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(project,qualified_name) DO UPDATE SET
                    label=excluded.label, name=excluded.name, file_path=excluded.file_path,
                    start_line=excluded.start_line, end_line=excluded.end_line,
                    properties=excluded.properties, docstring=excluded.docstring
                 RETURNING id",
            )?;
            for node in write.nodes {
                let props = serde_json::to_string(&node.properties)?;
                let doc = node
                    .properties
                    .get("docstring")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id: i64 = node_stmt.query_row(
                    params![
                        write.project,
                        node.label,
                        node.name,
                        node.qualified_name,
                        node.file_path,
                        node.start_line,
                        node.end_line,
                        props,
                        doc
                    ],
                    |row| row.get(0),
                )?;
                qn_to_id.insert(node.qualified_name.clone(), id);
                if !write.replace_nodes {
                    delta_touched.insert(id);
                }
                inserted_nodes += 1;
            }
        }

        phase!("nodes");
        // FTS rows
        {
            let mut fts_stmt = tx.prepare(
                "INSERT INTO nodes_fts(rowid,name_tokens,qn_tokens,label,file_path,body)
                 VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for node in write.nodes {
                let id = *qn_to_id
                    .get(node.qualified_name.as_str())
                    .expect("node inserted");
                let name_tokens = token_join(&node.name);
                let qn_tokens = token_join(&node.qualified_name);
                let body = node
                    .properties
                    .get("docstring")
                    .and_then(Value::as_str)
                    .map(token_join)
                    .unwrap_or_default();
                fts_stmt.execute(params![
                    id,
                    name_tokens,
                    qn_tokens,
                    node.label,
                    node.file_path,
                    body
                ])?;
            }
        }

        phase!("fts");
        let mut inserted_edges = 0i64;
        {
            let mut edge_stmt = tx.prepare(
                "INSERT INTO edges(project,source_id,target_id,type,properties)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(source_id,target_id,type,local_name_gen) DO UPDATE SET
                    properties=excluded.properties",
            )?;
            for edge in write.edges {
                let source = resolve_edge_endpoint(&tx, write, &mut qn_to_id, &edge.source_qn)?;
                let target = resolve_edge_endpoint(&tx, write, &mut qn_to_id, &edge.target_qn)?;
                let (Some(source), Some(target)) = (source, target) else {
                    continue;
                };
                let props = serde_json::to_string(&edge.properties)?;
                inserted_edges += edge_stmt.execute(params![
                    write.project,
                    source,
                    target,
                    edge.edge_type,
                    props
                ])? as i64;
                if !write.replace_nodes {
                    delta_touched.insert(source);
                    delta_touched.insert(target);
                }
            }
        }

        {
            let mut vec_stmt = tx.prepare(
                "INSERT INTO node_vectors(node_id,project,vector) VALUES(?1,?2,?3)
                 ON CONFLICT(node_id) DO UPDATE SET vector=excluded.vector",
            )?;
            for (qn, vector) in write.vectors {
                if let Some(&id) = qn_to_id.get(qn.as_str()) {
                    vec_stmt.execute(params![
                        id,
                        write.project,
                        gry_core::nomic::i8_to_blob(vector)
                    ])?;
                }
            }
        }

        {
            let mut cache_stmt = tx.prepare(
                "INSERT INTO parse_cache(project,rel_path,sha256,data) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(project,rel_path) DO UPDATE SET sha256=excluded.sha256,data=excluded.data",
            )?;
            for (rel_path, sha, data) in write.parse_cache {
                cache_stmt.execute(params![write.project, rel_path, sha, data])?;
            }
        }

        phase!("edges");
        {
            let mut sym_stmt = tx.prepare(
                "INSERT INTO symbols(project,qn,name,label,file_path,language,module_qn,scope,start_line,is_test)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for symbol in write.symbols {
                let scope = serde_json::to_string(&symbol.scope)?;
                sym_stmt.execute(params![
                    write.project,
                    symbol.qn,
                    symbol.name,
                    symbol.label,
                    symbol.file_path,
                    symbol.language,
                    symbol.module_qn,
                    scope,
                    symbol.start_line,
                    symbol.is_test as i64
                ])?;
            }
        }
        {
            let mut ref_stmt = tx.prepare(
                "INSERT INTO symbol_refs(project,file_path,name,kind) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(project,file_path,name,kind) DO NOTHING",
            )?;
            for reference in write.refs {
                ref_stmt.execute(params![
                    write.project,
                    reference.file_path,
                    reference.name,
                    reference.kind
                ])?;
            }
        }
        {
            let mut meta_stmt = tx.prepare(
                "INSERT INTO file_meta(project,rel_path,language,module_is_dir) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(project,rel_path) DO UPDATE SET language=excluded.language,
                    module_is_dir=excluded.module_is_dir",
            )?;
            for meta in write.file_meta {
                meta_stmt.execute(params![
                    write.project,
                    meta.rel_path,
                    meta.language,
                    meta.module_is_dir as i64
                ])?;
            }
        }

        phase!("symbols/refs/meta");
        if !write.replace_nodes {
            let mut stale: Vec<&str> = write.affected_files.iter().map(String::as_str).collect();
            stale.extend(write.deleted_files.iter().map(String::as_str));
            let mut del =
                tx.prepare("DELETE FROM index_coverage WHERE project=?1 AND rel_path=?2")?;
            for file in stale {
                del.execute(params![write.project, file])?;
            }
        }
        {
            let mut cov_stmt = tx.prepare(
                "INSERT INTO index_coverage(project,rel_path,kind,detail) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(project,rel_path,kind) DO UPDATE SET detail=excluded.detail",
            )?;
            for row in write.coverage {
                cov_stmt.execute(params![write.project, row.rel_path, row.kind, row.detail])?;
            }
        }

        if let Some(meta) = write.coverage_meta {
            tx.execute(
                "INSERT INTO index_coverage_meta(project,generation,index_mode,recorded_at,recording_status,ignored_files_stored,ignored_files_total,coverage_version,hash_records_complete)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(project) DO UPDATE SET generation=excluded.generation,
                   index_mode=excluded.index_mode, recorded_at=excluded.recorded_at,
                   recording_status=excluded.recording_status,
                   ignored_files_stored=excluded.ignored_files_stored,
                   ignored_files_total=excluded.ignored_files_total,
                   coverage_version=excluded.coverage_version,
                   hash_records_complete=excluded.hash_records_complete",
                params![
                    write.project,
                    meta.generation,
                    meta.index_mode,
                    meta.recorded_at,
                    meta.recording_status,
                    meta.ignored_files_stored,
                    meta.ignored_files_total,
                    meta.coverage_version,
                    meta.hash_records_complete as i64
                ],
            )?;
        }

        phase!("coverage");
        if !write.replace_nodes {
            let branch_qns: Vec<String> = write
                .nodes
                .iter()
                .filter(|n| n.label == "Branch")
                .map(|n| n.qualified_name.clone())
                .collect();
            let structural_change =
                !write.deleted_files.is_empty() || !write.removed_qns.is_empty();
            gc_delta(
                &tx,
                write.project,
                &branch_qns,
                structural_change,
                write.gc_globals,
                self.fts_delete,
            )?;
            phase!("gc");
            if !write.nodes_only {
                recompute_importance(&tx, write.project, &delta_touched)?;
            }
            phase!("importance");
        }

        tx.commit()?;
        phase!("commit");
        let _ = phase_t;
        Ok(PublishStats {
            nodes: inserted_nodes,
            edges: inserted_edges,
        })
    }

    // ── Nodes and edges ──────────────────────────────────────────────

    pub fn integrity_check(&self) -> StoreResult<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn has_fts(&self) -> StoreResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='nodes_fts'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Delta publication requires FTS5 contentless-delete support.
    pub fn supports_delta(&self) -> bool {
        self.fts_delete
    }

    pub fn symbols(&self, project: &str) -> StoreResult<Vec<SymbolRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT qn,name,label,file_path,language,module_qn,scope,start_line,is_test
             FROM symbols WHERE project=?1 ORDER BY qn",
        )?;
        let rows = stmt.query_map([project], |row| {
            let scope: String = row.get(6)?;
            Ok(SymbolRow {
                qn: row.get(0)?,
                name: row.get(1)?,
                label: row.get(2)?,
                file_path: row.get(3)?,
                language: row.get(4)?,
                module_qn: row.get(5)?,
                scope: serde_json::from_str(&scope).unwrap_or_default(),
                start_line: row.get(7)?,
                is_test: row.get::<_, i64>(8)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn symbols_for_files(
        &self,
        project: &str,
        files: &[String],
    ) -> StoreResult<Vec<SymbolRow>> {
        let mut out = Vec::new();
        for chunk in files.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let ph = placeholders(chunk.len());
            let sql = format!(
                "SELECT qn,name,label,file_path,language,module_qn,scope,start_line,is_test
                 FROM symbols WHERE project=? AND file_path IN ({ph})"
            );
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            values.push(&project);
            for file in chunk {
                values.push(file);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(values.as_slice(), |row| {
                let scope: String = row.get(6)?;
                Ok(SymbolRow {
                    qn: row.get(0)?,
                    name: row.get(1)?,
                    label: row.get(2)?,
                    file_path: row.get(3)?,
                    language: row.get(4)?,
                    module_qn: row.get(5)?,
                    scope: serde_json::from_str(&scope).unwrap_or_default(),
                    start_line: row.get(7)?,
                    is_test: row.get::<_, i64>(8)? != 0,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        out.sort_by(|a, b| a.qn.cmp(&b.qn));
        Ok(out)
    }

    pub fn all_qns(&self, project: &str) -> StoreResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT qualified_name FROM nodes WHERE project=?1")?;
        let rows = stmt.query_map([project], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn parse_cache_entry(
        &self,
        project: &str,
        rel_path: &str,
    ) -> StoreResult<Option<(String, Vec<u8>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT sha256,data FROM parse_cache WHERE project=?1 AND rel_path=?2",
                params![project, rel_path],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?)
    }

    /// All import references, used to detect files affected by new or removed
    /// modules (an existing unresolved import may become resolvable).
    pub fn import_refs(&self, project: &str) -> StoreResult<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT file_path,name FROM symbol_refs WHERE project=?1 AND kind='import'")?;
        let rows = stmt.query_map([project], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn file_meta(&self, project: &str) -> StoreResult<Vec<FileMetaRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path,language,module_is_dir FROM file_meta WHERE project=?1 ORDER BY rel_path",
        )?;
        let rows = stmt.query_map([project], |row| {
            Ok(FileMetaRow {
                rel_path: row.get(0)?,
                language: row.get(1)?,
                module_is_dir: row.get::<_, i64>(2)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn qns_for_files(&self, project: &str, files: &[String]) -> StoreResult<Vec<String>> {
        let mut out = Vec::new();
        for chunk in files.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let ph = placeholders(chunk.len());
            let sql =
                format!("SELECT qualified_name FROM nodes WHERE project=? AND file_path IN ({ph})");
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            values.push(&project);
            for file in chunk {
                values.push(file);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(values.as_slice(), |row| row.get::<_, String>(0))?;
            for qn in rows {
                out.push(qn?);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn node_ids_for_files(&self, project: &str, files: &[String]) -> StoreResult<Vec<i64>> {
        let mut out = Vec::new();
        for chunk in files.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let ph = placeholders(chunk.len());
            let sql = format!("SELECT id FROM nodes WHERE project=? AND file_path IN ({ph})");
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            values.push(&project);
            for file in chunk {
                values.push(file);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(values.as_slice(), |row| row.get::<_, i64>(0))?;
            for id in rows {
                out.push(id?);
            }
        }
        Ok(out)
    }

    /// Files whose edges point at a node in `files` (reverse dependencies).
    pub fn dependent_files(&self, project: &str, files: &[String]) -> StoreResult<Vec<String>> {
        let mut out = Vec::new();
        for chunk in files.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let ph = placeholders(chunk.len());
            let sql = format!(
                "SELECT DISTINCT sn.file_path FROM edges e
                 JOIN nodes sn ON sn.id=e.source_id
                 JOIN nodes tn ON tn.id=e.target_id
                 WHERE e.project=? AND tn.project=? AND tn.file_path IN ({ph})
                   AND sn.file_path <> ''"
            );
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 2);
            values.push(&project);
            values.push(&project);
            for file in chunk {
                values.push(file);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(values.as_slice(), |row| row.get::<_, String>(0))?;
            for file in rows {
                out.push(file?);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Files that reference any of `names` (calls, usages, imports, bases,
    /// decorators). Used to bound incremental re-resolution.
    pub fn files_for_ref_names(&self, project: &str, names: &[String]) -> StoreResult<Vec<String>> {
        let mut out = Vec::new();
        for chunk in names.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let ph = placeholders(chunk.len());
            let sql = format!(
                "SELECT DISTINCT file_path FROM symbol_refs WHERE project=? AND name IN ({ph})"
            );
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            values.push(&project);
            for name in chunk {
                values.push(name);
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(values.as_slice(), |row| row.get::<_, String>(0))?;
            for file in rows {
                out.push(file?);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn node_by_qn(&self, project: &str, qn: &str) -> StoreResult<Option<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND qualified_name=?2",
        )?;
        Ok(stmt
            .query_row(params![project, qn], row_to_node)
            .optional()?)
    }

    /// Resolve a full QN, then a unique suffix, then an exact short name.
    pub fn resolve_node(&self, project: &str, name_or_qn: &str) -> StoreResult<Option<Node>> {
        if let Some(node) = self.node_by_qn(project, name_or_qn)? {
            return Ok(Some(node));
        }
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND qualified_name LIKE ?2
             ORDER BY length(qualified_name), qualified_name LIMIT 8",
        )?;
        let suffix = format!("%.{name_or_qn}");
        let rows = stmt.query_map(params![project, suffix], row_to_node)?;
        let found: Vec<Node> = rows.collect::<Result<Vec<_>, _>>()?;
        if found.len() == 1 {
            return Ok(found.into_iter().next());
        }
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND name=?2
             ORDER BY CASE label WHEN 'Function' THEN 0 WHEN 'Method' THEN 1 ELSE 2 END,
                      qualified_name LIMIT 8",
        )?;
        let rows = stmt.query_map(params![project, name_or_qn], row_to_node)?;
        let found: Vec<Node> = rows.collect::<Result<Vec<_>, _>>()?;
        if found.len() == 1 {
            return Ok(found.into_iter().next());
        }
        Ok(None)
    }

    pub fn nodes_by_file(&self, project: &str, file_path: &str) -> StoreResult<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND file_path=?2 ORDER BY start_line,end_line,label",
        )?;
        let rows = stmt.query_map(params![project, file_path], row_to_node)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn all_nodes(&self, project: &str) -> StoreResult<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([project], row_to_node)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn all_edges(&self, project: &str) -> StoreResult<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,source_id,target_id,type,properties
             FROM edges WHERE project=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([project], |row| {
            Ok(Edge {
                id: row.get(0)?,
                project: row.get(1)?,
                source_id: row.get(2)?,
                target_id: row.get(3)?,
                edge_type: row.get(4)?,
                properties: parse_props(row.get::<_, String>(5)?),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn edges_for_nodes(
        &self,
        node_ids: &[i64],
        edge_types: &[String],
        direction: &str,
    ) -> StoreResult<Vec<Edge>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; node_ids.len()].join(",");
        let mut sql = String::new();
        match direction {
            "inbound" => sql.push_str(&format!(
                "SELECT id,project,source_id,target_id,type,properties FROM edges \
                 WHERE target_id IN ({placeholders})"
            )),
            "outbound" => sql.push_str(&format!(
                "SELECT id,project,source_id,target_id,type,properties FROM edges \
                 WHERE source_id IN ({placeholders})"
            )),
            _ => sql.push_str(&format!(
                "SELECT id,project,source_id,target_id,type,properties FROM edges \
                 WHERE source_id IN ({placeholders}) OR target_id IN ({placeholders})"
            )),
        }
        if !edge_types.is_empty() {
            let type_ph = vec!["?"; edge_types.len()].join(",");
            sql.push_str(&format!(" AND type IN ({type_ph})"));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let mut values: Vec<&dyn rusqlite::ToSql> = node_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        if direction == "both" {
            values.extend(node_ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        }
        for edge_type in edge_types {
            values.push(edge_type as &dyn rusqlite::ToSql);
        }
        let rows = stmt.query_map(values.as_slice(), |row| {
            Ok(Edge {
                id: row.get(0)?,
                project: row.get(1)?,
                source_id: row.get(2)?,
                target_id: row.get(3)?,
                edge_type: row.get(4)?,
                properties: parse_props(row.get::<_, String>(5)?),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn node_degree(&self, node_id: i64, edge_types: &[String]) -> StoreResult<(i64, i64)> {
        let (in_sql, out_sql) = if edge_types.is_empty() {
            (
                "SELECT COUNT(*) FROM edges WHERE target_id=?1".to_string(),
                "SELECT COUNT(*) FROM edges WHERE source_id=?1".to_string(),
            )
        } else {
            let ph = vec!["?"; edge_types.len()].join(",");
            (
                format!("SELECT COUNT(*) FROM edges WHERE target_id=?1 AND type IN ({ph})"),
                format!("SELECT COUNT(*) FROM edges WHERE source_id=?1 AND type IN ({ph})"),
            )
        };
        let mut in_params: Vec<&dyn rusqlite::ToSql> = vec![&node_id];
        let mut out_params: Vec<&dyn rusqlite::ToSql> = vec![&node_id];
        for t in edge_types {
            in_params.push(t);
            out_params.push(t);
        }
        let in_deg: i64 = self
            .conn
            .query_row(&in_sql, in_params.as_slice(), |row| row.get(0))?;
        let out_deg: i64 = self
            .conn
            .query_row(&out_sql, out_params.as_slice(), |row| row.get(0))?;
        Ok((in_deg, out_deg))
    }

    pub fn connected_names(
        &self,
        node_id: i64,
        relationship: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<String>> {
        let mut sql = String::from(
            "SELECT DISTINCT n.name FROM edges e JOIN nodes n ON n.id=e.target_id WHERE e.source_id=?1",
        );
        if relationship.is_some() {
            sql.push_str(" AND e.type=?2");
        }
        sql.push_str(" LIMIT ?3");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = if let Some(rel) = relationship {
            stmt.query_map(params![node_id, rel, limit as i64], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?
        } else {
            stmt.query_map(params![node_id, limit as i64], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?
        };
        Ok(rows)
    }

    pub fn vectors(&self, project: &str) -> StoreResult<Vec<(i64, Vec<i8>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT node_id,vector FROM node_vectors WHERE project=?1")?;
        let rows = stmt.query_map([project], |row| {
            let id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id, gry_core::nomic::blob_to_i8(&blob)))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ── Coverage / meta ──────────────────────────────────────────────

    pub fn coverage(&self, project: &str) -> StoreResult<Vec<CoverageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path,kind,detail FROM index_coverage WHERE project=?1
             ORDER BY rel_path,kind",
        )?;
        let rows = stmt.query_map([project], |row| {
            Ok(CoverageRow {
                rel_path: row.get(0)?,
                kind: row.get(1)?,
                detail: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn coverage_meta(&self, project: &str) -> StoreResult<Option<CoverageMeta>> {
        Ok(self
            .conn
            .query_row(
                "SELECT generation,index_mode,recorded_at,recording_status,ignored_files_stored,
                        ignored_files_total,coverage_version,hash_records_complete
                 FROM index_coverage_meta WHERE project=?1",
                [project],
                |row| {
                    Ok(CoverageMeta {
                        generation: row.get(0)?,
                        index_mode: row.get(1)?,
                        recorded_at: row.get(2)?,
                        recording_status: row.get(3)?,
                        ignored_files_stored: row.get(4)?,
                        ignored_files_total: row.get(5)?,
                        coverage_version: row.get(6)?,
                        hash_records_complete: row.get::<_, i64>(7)? != 0,
                    })
                },
            )
            .optional()?)
    }

    // ── Parse cache ──────────────────────────────────────────────────

    pub fn parse_cache(&self, project: &str) -> StoreResult<Vec<(String, String, Vec<u8>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path,sha256,data FROM parse_cache WHERE project=?1 ORDER BY rel_path",
        )?;
        let rows = stmt.query_map([project], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ── ADR (project_summaries) ──────────────────────────────────────

    pub fn adr_get(&self, project: &str) -> StoreResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT summary FROM project_summaries WHERE project=?1",
                [project],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn adr_put(&self, project: &str, content: &str) -> StoreResult<()> {
        let now = now_iso();
        let hash = gry_core::config::resolve_cache_dir()
            .to_string_lossy()
            .to_string();
        let _ = hash;
        let digest = sha256_hex(content.as_bytes());
        self.conn.execute(
            "INSERT INTO project_summaries(project,summary,source_hash,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?4)
             ON CONFLICT(project) DO UPDATE SET summary=excluded.summary,
               source_hash=excluded.source_hash, updated_at=excluded.updated_at",
            params![project, content, digest, now],
        )?;
        Ok(())
    }

    // ── Traces ───────────────────────────────────────────────────────

    pub fn upsert_observed_call(
        &self,
        project: &str,
        caller_qn: &str,
        callee_qn: &str,
        label: &str,
        count: i64,
    ) -> StoreResult<()> {
        let now = now_iso();
        self.conn.execute(
            "INSERT INTO observed_calls(project,caller_qn,callee_qn,label,count,first_seen,last_seen)
             VALUES(?1,?2,?3,?4,?5,?6,?6)
             ON CONFLICT(project,caller_qn,callee_qn,label) DO UPDATE SET
               count=count+excluded.count, last_seen=excluded.last_seen",
            params![project, caller_qn, callee_qn, label, count, now],
        )?;
        Ok(())
    }

    pub fn upsert_observed_path(
        &self,
        project: &str,
        label: &str,
        path_json: &str,
        count: i64,
    ) -> StoreResult<()> {
        let now = now_iso();
        self.conn.execute(
            "INSERT INTO observed_paths(project,label,path_json,count,first_seen,last_seen)
             VALUES(?1,?2,?3,?4,?5,?5)
             ON CONFLICT(project,label,path_json) DO UPDATE SET
               count=count+excluded.count, last_seen=excluded.last_seen",
            params![project, label, path_json, count, now],
        )?;
        Ok(())
    }

    pub fn prune_observed_labels(&self, project: &str, keep: usize) -> StoreResult<()> {
        let excess: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT label FROM observed_calls WHERE project=?1 GROUP BY label
                 ORDER BY MAX(last_seen) DESC LIMIT -1 OFFSET ?2",
            )?;
            let rows = stmt.query_map(params![project, keep as i64], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()?
        };
        for label in excess {
            self.conn.execute(
                "DELETE FROM observed_calls WHERE project=?1 AND label=?2",
                params![project, label],
            )?;
            self.conn.execute(
                "DELETE FROM observed_paths WHERE project=?1 AND label=?2",
                params![project, label],
            )?;
        }
        Ok(())
    }

    pub fn observed_calls(
        &self,
        project: &str,
        label: Option<&str>,
    ) -> StoreResult<Vec<(String, String, String, i64)>> {
        let (sql, use_label) = match label {
            Some(_) => (
                "SELECT caller_qn,callee_qn,label,count FROM observed_calls
                 WHERE project=?1 AND label=?2 ORDER BY caller_qn,callee_qn",
                true,
            ),
            None => (
                "SELECT caller_qn,callee_qn,label,count FROM observed_calls
                 WHERE project=?1 ORDER BY caller_qn,callee_qn",
                false,
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = if use_label {
            stmt.query_map(params![project, label.unwrap()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([project], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn observed_paths(
        &self,
        project: &str,
        label: Option<&str>,
    ) -> StoreResult<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT label,path_json,count FROM observed_paths
             WHERE project=?1 AND (?2 IS NULL OR label=?2) ORDER BY label,path_json",
        )?;
        let rows = stmt.query_map(params![project, label], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Insert a cross-project edge on `project` (the source project's store row).
    /// Returns true when a new edge was inserted or updated.
    pub fn insert_cross_edge(
        &self,
        project: &str,
        source_qn: &str,
        target_project: &str,
        target_qn: &str,
        edge_type: &str,
        properties: Value,
    ) -> StoreResult<bool> {
        let source_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM nodes WHERE project=?1 AND qualified_name=?2",
                params![project, source_qn],
                |row| row.get(0),
            )
            .optional()?;
        let target_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM nodes WHERE project=?1 AND qualified_name=?2",
                params![target_project, target_qn],
                |row| row.get(0),
            )
            .optional()?;
        let (Some(source_id), Some(target_id)) = (source_id, target_id) else {
            return Ok(false);
        };
        let props = serde_json::to_string(&properties)?;
        let changed = self.conn.execute(
            "INSERT INTO edges(project,source_id,target_id,type,properties)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(source_id,target_id,type,local_name_gen) DO UPDATE SET
                properties=excluded.properties",
            params![project, source_id, target_id, edge_type, props],
        )?;
        Ok(changed > 0)
    }

    pub fn get_meta(&self, key: &str) -> StoreResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT v FROM store_meta WHERE k=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO store_meta(k,v) VALUES(?1,?2)
             ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            [key, value],
        )?;
        Ok(())
    }

    pub fn checkpoint(&self) -> StoreResult<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;")?;
        Ok(())
    }

    /// Cheap post-commit maintenance for incremental updates.
    pub fn checkpoint_passive(&self) -> StoreResult<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PublishStats {
    pub nodes: i64,
    pub edges: i64,
}

fn configure(conn: &Connection) -> StoreResult<()> {
    conn.busy_timeout(Duration::from_secs(15))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA temp_store=MEMORY;
         PRAGMA cache_size=-65536;",
    )?;
    Ok(())
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

fn delete_outgoing_for_files(
    tx: &Transaction<'_>,
    project: &str,
    files: &[String],
) -> StoreResult<()> {
    for chunk in files.chunks(400) {
        if chunk.is_empty() {
            continue;
        }
        let ph = placeholders(chunk.len());
        let sql = format!(
            "DELETE FROM edges WHERE source_id IN (SELECT id FROM nodes WHERE project=? AND file_path IN ({ph}))
             AND type NOT IN ('FILE_CHANGES_WITH','SIMILAR_TO','SEMANTICALLY_RELATED')
             AND type NOT LIKE 'CROSS_%'"
        );
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        values.push(&project);
        for file in chunk {
            values.push(file);
        }
        tx.execute(&sql, values.as_slice())?;
    }
    Ok(())
}

fn delete_qns(tx: &Transaction<'_>, project: &str, qns: &[String]) -> StoreResult<()> {
    for chunk in qns.chunks(400) {
        if chunk.is_empty() {
            continue;
        }
        let ph = placeholders(chunk.len());
        let sql = format!("DELETE FROM nodes WHERE project=? AND qualified_name IN ({ph})");
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        values.push(&project);
        for qn in chunk {
            values.push(qn);
        }
        tx.execute(&sql, values.as_slice())?;
    }
    Ok(())
}

fn delete_fts_for_files(tx: &Transaction<'_>, project: &str, files: &[String]) -> StoreResult<()> {
    for chunk in files.chunks(400) {
        if chunk.is_empty() {
            continue;
        }
        let ph = placeholders(chunk.len());
        let sql = format!(
            "DELETE FROM nodes_fts WHERE rowid IN (SELECT id FROM nodes WHERE project=? AND file_path IN ({ph}))"
        );
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        values.push(&project);
        for file in chunk {
            values.push(file);
        }
        tx.execute(&sql, values.as_slice())?;
    }
    Ok(())
}

/// Delete nodes whose file is no longer in the project's current file set.
/// `file_hashes` must already contain the new set. Edges and vectors cascade
/// from the node rows; FTS is contentless and must be deleted explicitly.
fn delete_orphan_nodes(tx: &Transaction<'_>, project: &str) -> StoreResult<usize> {
    let orphans: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT file_path FROM nodes
             WHERE project=?1 AND file_path<>''
               AND file_path NOT IN (SELECT rel_path FROM file_hashes WHERE project=?1)",
        )?;
        let rows = stmt.query_map([project], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if orphans.is_empty() {
        return Ok(0);
    }
    delete_fts_for_files(tx, project, &orphans)?;
    for chunk in orphans.chunks(400) {
        let ph = placeholders(chunk.len());
        let sql = format!("DELETE FROM nodes WHERE project=? AND file_path IN ({ph})");
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        values.push(&project);
        for file in chunk {
            values.push(file);
        }
        tx.execute(&sql, values.as_slice())?;
    }
    let refs: Vec<&str> = orphans.iter().map(String::as_str).collect();
    delete_file_rows(tx, project, &refs)?;
    Ok(orphans.len())
}

fn delete_file_rows(tx: &Transaction<'_>, project: &str, files: &[&str]) -> StoreResult<()> {
    for chunk in files.chunks(400) {
        if chunk.is_empty() {
            continue;
        }
        let ph = placeholders(chunk.len());
        for table in [
            "file_hashes",
            "parse_cache",
            "file_meta",
            "symbols",
            "symbol_refs",
        ] {
            let column = if table == "symbols" || table == "symbol_refs" {
                "file_path"
            } else {
                "rel_path"
            };
            let sql = format!("DELETE FROM {table} WHERE project=? AND {column} IN ({ph})");
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            values.push(&project);
            for file in chunk {
                values.push(file);
            }
            tx.execute(&sql, values.as_slice())?;
        }
    }
    Ok(())
}

fn collect_delta_touched(
    tx: &Transaction<'_>,
    write: &ProjectWrite<'_>,
    touched: &mut HashSet<i64>,
) -> StoreResult<()> {
    for chunk in write.affected_files.chunks(400) {
        if chunk.is_empty() {
            continue;
        }
        let ph = placeholders(chunk.len());
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        values.push(&write.project);
        for file in chunk {
            values.push(file);
        }
        let sql = format!("SELECT DISTINCT id FROM nodes WHERE project=? AND file_path IN ({ph})");
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(values.as_slice(), |row| row.get::<_, i64>(0))?;
        for id in rows {
            touched.insert(id?);
        }
    }
    // Inbound edges to removed nodes affect their sources' importance.
    for chunk in write.removed_qns.chunks(400) {
        if chunk.is_empty() {
            continue;
        }
        let ph = placeholders(chunk.len());
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 2);
        values.push(&write.project);
        values.push(&write.project);
        for qn in chunk {
            values.push(qn);
        }
        let sql = format!(
            "SELECT DISTINCT e.source_id FROM edges e JOIN nodes n ON n.id=e.source_id
             WHERE n.project=? AND e.target_id IN (SELECT id FROM nodes WHERE project=? AND qualified_name IN ({ph}))"
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(values.as_slice(), |row| row.get::<_, i64>(0))?;
        for id in rows {
            touched.insert(id?);
        }
    }
    Ok(())
}

fn resolve_edge_endpoint(
    tx: &Transaction<'_>,
    write: &ProjectWrite<'_>,
    cache: &mut HashMap<String, i64>,
    qn: &str,
) -> StoreResult<Option<i64>> {
    if let Some(id) = cache.get(qn) {
        return Ok(Some(*id));
    }
    if write.replace_nodes {
        return Ok(None);
    }
    let id: Option<i64> = tx
        .query_row(
            "SELECT id FROM nodes WHERE project=?1 AND qualified_name=?2",
            params![write.project, qn],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = id {
        cache.insert(qn.to_string(), id);
    }
    Ok(id)
}

fn gc_delta(
    tx: &Transaction<'_>,
    project: &str,
    branch_qns: &[String],
    structural_change: bool,
    gc_globals: bool,
    fts_delete: bool,
) -> StoreResult<()> {
    // Branch nodes are a projection of the current checkout; stale branches
    // must not survive an update.
    if branch_qns.is_empty() {
        tx.execute(
            "DELETE FROM nodes WHERE project=?1 AND label='Branch'",
            [project],
        )?;
    } else {
        for chunk in branch_qns.chunks(400) {
            let ph = placeholders(chunk.len());
            let sql = format!(
                "DELETE FROM nodes WHERE project=?1 AND label='Branch' AND qualified_name NOT IN ({ph})"
            );
            let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
            values.push(&project);
            for qn in chunk {
                values.push(qn);
            }
            tx.execute(&sql, values.as_slice())?;
        }
    }
    // Global infrastructure nodes with no remaining edge are orphans.
    if gc_globals {
        let orphan_globals = "project=?1 AND label IN
             ('Package','EnvVar','Channel','Process','Image','Resource','Chart')
             AND id NOT IN (SELECT source_id FROM edges WHERE project=?1)
             AND id NOT IN (SELECT target_id FROM edges WHERE project=?1)";
        if fts_delete {
            let sql = format!(
                "DELETE FROM nodes_fts WHERE rowid IN (SELECT id FROM nodes WHERE {orphan_globals})"
            );
            tx.execute(&sql, [project])?;
        }
        tx.execute(
            &format!("DELETE FROM nodes WHERE {orphan_globals}"),
            [project],
        )?;
    }
    // Empty folders only change when files appear or disappear.
    if structural_change {
        for _ in 0..4 {
            let changed = tx.execute(
                "DELETE FROM nodes WHERE project=?1 AND label='Folder'
                 AND id NOT IN (SELECT source_id FROM edges WHERE project=?1 AND type IN ('CONTAINS_FILE','CONTAINS_FOLDER'))",
                [project],
            )?;
            if changed == 0 {
                break;
            }
        }
    }
    Ok(())
}

fn recompute_importance(
    tx: &Transaction<'_>,
    project: &str,
    touched: &HashSet<i64>,
) -> StoreResult<()> {
    let types = "'CALLS','USAGE','CALL_REFERENCE','INHERITS','IMPLEMENTS','HTTP_CALLS'";
    let mut ids: Vec<i64> = touched.iter().copied().collect();
    ids.sort_unstable();
    let mut set = tx.prepare(
        "UPDATE nodes SET properties=json_set(properties,'$.importance',?2) WHERE id=?1",
    )?;
    let mut remove = tx.prepare(
        "UPDATE nodes SET properties=json_remove(properties,'$.importance') WHERE id=?1
         AND json_extract(properties,'$.importance') IS NOT NULL",
    )?;
    for id in ids {
        let sql = format!(
            "SELECT
               (SELECT COUNT(*) FROM edges WHERE target_id=?1 AND type IN ({types}))*2 +
               (SELECT COUNT(*) FROM edges WHERE source_id=?1 AND type IN ({types}))"
        );
        let score: i64 = tx.query_row(&sql, [id], |row| row.get(0))?;
        if score > 0 {
            set.execute(params![id, score])?;
        } else {
            remove.execute([id])?;
        }
    }
    let _ = project;
    Ok(())
}

fn clear_fts_project(tx: &Transaction<'_>, project: &str, fts_delete: bool) -> StoreResult<()> {
    if fts_delete {
        tx.execute(
            "DELETE FROM nodes_fts WHERE rowid IN (SELECT id FROM nodes WHERE project=?1)",
            [project],
        )?;
    } else {
        // Contentless FTS without delete support: rebuild the entire index.
        tx.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('delete-all')", [])?;
        let mut stmt = tx.prepare(
            "SELECT id,name,qualified_name,label,file_path,
                    coalesce(json_extract(properties,'$.docstring'),'')
             FROM nodes ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut insert = tx.prepare(
            "INSERT INTO nodes_fts(rowid,name_tokens,qn_tokens,label,file_path,body)
             VALUES(?1,?2,?3,?4,?5,?6)",
        )?;
        for row in rows {
            let (id, name, qn, label, path, body) = row?;
            insert.execute(params![
                id,
                token_join(&name),
                token_join(&qn),
                label,
                path,
                token_join(&body)
            ])?;
        }
    }
    Ok(())
}

pub(crate) fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    Ok(Node {
        id: row.get(0)?,
        project: row.get(1)?,
        label: row.get(2)?,
        name: row.get(3)?,
        qualified_name: row.get(4)?,
        file_path: row.get(5)?,
        start_line: row.get(6)?,
        end_line: row.get(7)?,
        properties: parse_props(row.get::<_, String>(8)?),
    })
}

fn parse_props(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or_else(|_| Value::Object(Map::new()))
}

pub(crate) fn token_join(text: &str) -> String {
    let mut tokens = split_identifier(text);
    tokens.extend(split_text(text));
    tokens.sort();
    tokens.dedup();
    tokens.join(" ")
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn new_generation() -> String {
    format!(
        "{}-{}",
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn labels_all() -> &'static [&'static str] {
    labels::ALL
}

pub fn edge_types_all() -> &'static [&'static str] {
    gry_core::types::edges::ALL
}

/// Convert a node to its serialized property object for comparisons.
pub fn edge_identity(edge: &Edge, nodes: &HashMap<i64, &Node>) -> String {
    let source = nodes
        .get(&edge.source_id)
        .map(|n| n.qualified_name.as_str())
        .unwrap_or("");
    let target = nodes
        .get(&edge.target_id)
        .map(|n| n.qualified_name.as_str())
        .unwrap_or("");
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        source,
        target,
        edge.edge_type,
        edge.local_name().unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_store;

    #[test]
    fn delta_publish_removes_nodes_for_files_no_longer_present() {
        let mut store = test_store();
        let files = vec![FileHash {
            rel_path: "src/main.rs".into(),
            sha256: "a2".into(),
            mtime_ns: 2,
            size: 20,
        }];
        let deleted = vec!["src/lib.rs".to_string(), "tests/test_main.rs".to_string()];
        let mut write = ProjectWrite::new("p", "/tmp/p", "full", "gen-2");
        write.files = &files;
        write.replace_nodes = false;
        write.deleted_files = &deleted;
        store.publish(&write).unwrap();

        assert!(
            store.nodes_by_file("p", "src/lib.rs").unwrap().is_empty(),
            "deleted file kept its nodes"
        );
        assert!(
            store
                .nodes_by_file("p", "tests/test_main.rs")
                .unwrap()
                .is_empty(),
            "deleted test file kept its nodes"
        );
        assert!(
            !store.nodes_by_file("p", "src/main.rs").unwrap().is_empty(),
            "surviving file lost its nodes"
        );
    }
}
