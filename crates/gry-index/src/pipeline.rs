//! Full and incremental indexing pipeline.

use crate::build::{build, BuildOptions, ParsedFile};
use crate::discover::{discover, DiscoverOptions, DiscoveredFile, IndexMode};
use gry_core::config::Config;
use gry_core::fqn::project_name_from_path;
use gry_core::types::FileHash;
use gry_parse::extract_file;
use gry_parse::model::FileExtract;
use gry_store::{CoverageMeta, CoverageRow, ProjectWrite, Store};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub(crate) fn timing_enabled() -> bool {
    std::env::var_os("GRY_TIMING").is_some()
}

#[macro_export]
macro_rules! phase {
    ($label:expr, $started:expr) => {
        if $crate::pipeline::timing_enabled() {
            eprintln!(
                "[gry] {:<28} {:>7} ms",
                $label,
                $started.elapsed().as_millis()
            );
        }
    };
}

pub const EXTRACTOR_VERSION: &str = "25";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRequest {
    pub repo_path: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub persistence: bool,
    #[serde(default)]
    pub doc_candidates: Option<bool>,
    #[serde(default)]
    pub target_projects: Vec<String>,
}

fn default_mode() -> String {
    "full".to_string()
}

impl IndexRequest {
    pub fn new(repo_path: impl Into<String>) -> Self {
        Self {
            repo_path: repo_path.into(),
            mode: "full".to_string(),
            name: None,
            persistence: false,
            doc_candidates: None,
            target_projects: Vec::new(),
        }
    }

    pub fn mode(&self) -> IndexMode {
        IndexMode::parse(&self.mode).unwrap_or(IndexMode::Full)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexReport {
    pub project: String,
    pub root_path: String,
    pub mode: String,
    pub generation: String,
    pub files: usize,
    pub source_bytes: i64,
    pub nodes: i64,
    pub edges: i64,
    pub parsed_files: usize,
    pub reused_files: usize,
    pub deleted_files: usize,
    pub duration_ms: u128,
    pub unchanged: bool,
    pub skipped: Vec<SkippedFile>,
    pub excluded: ExcludedReport,
    pub parse_partial: Vec<SkippedFile>,
    pub coverage: CoverageSummary,
    pub artifact: Option<String>,
    pub artifact_error: Option<String>,
    pub cross_repo: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
    #[serde(default)]
    pub phase: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExcludedReport {
    pub dirs: Vec<String>,
    pub count: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub gaps: usize,
    pub parse_partial: usize,
    pub ignored_files: usize,
    pub ignored_total: usize,
    pub hash_records_complete: bool,
}

pub struct Indexer {
    pub config: Config,
}

impl Indexer {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn index(&self, request: &IndexRequest) -> gry_core::GryResult<IndexReport> {
        self.index_with_cancel(request, Arc::new(AtomicBool::new(false)))
    }

    pub fn index_with_cancel(
        &self,
        request: &IndexRequest,
        cancel: Arc<AtomicBool>,
    ) -> gry_core::GryResult<IndexReport> {
        let started = Instant::now();
        let root = self.config.resolve_index_root(&request.repo_path)?;
        let mode = request.mode();
        if mode == IndexMode::CrossRepoIntelligence {
            return crate::cross_repo::run_cross_repo(&self.config, request, &cancel, started);
        }
        let project = request
            .name
            .as_deref()
            .map(gry_core::fqn::sanitize_project_name)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| project_name_from_path(root.to_string_lossy().as_ref()));

        let _lock = ProjectLock::acquire(&self.config, &project)?;
        let mut store = Store::open(&self.config.db_path)?;
        let existing = store.project(&project)?;
        let known_hashes: HashMap<String, FileHash> = store
            .file_hashes(&project)?
            .into_iter()
            .map(|h| (h.rel_path.clone(), h))
            .collect();
        let existing_files: HashSet<String> = known_hashes.keys().cloned().collect();
        let discover_options = DiscoverOptions {
            mode,
            max_files: self.config.max_files,
            max_source_mb: self.config.max_source_mb,
            max_file_bytes: 64 * 1024 * 1024,
        };
        let t = Instant::now();
        let discovery = discover(&root, &discover_options, &known_hashes, &cancel)?;
        phase!("discovery", t);
        if cancel.load(Ordering::Relaxed) {
            return Err(gry_core::GryError::Cancelled);
        }
        if let Some(violation) = &discovery.limit_violation {
            return Err(gry_core::GryError::invalid(violation.clone()));
        }

        let discovered_map: HashMap<String, &DiscoveredFile> = discovery
            .files
            .iter()
            .map(|f| (f.rel_path.clone(), f))
            .collect();
        let deleted: Vec<String> = existing_files
            .iter()
            .filter(|p| !discovered_map.contains_key(*p))
            .cloned()
            .collect();
        let changed: Vec<&DiscoveredFile> = discovery
            .files
            .iter()
            .filter(|f| {
                known_hashes
                    .get(&f.rel_path)
                    .map(|h| h.sha256 != f.sha256 || h.size != f.size)
                    .unwrap_or(true)
            })
            .collect();
        let mode_changed = existing
            .as_ref()
            .map(|p| p.mode != mode.as_str())
            .unwrap_or(true);
        let extractor_ok =
            store.get_meta("extractor_version")?.as_deref() == Some(EXTRACTOR_VERSION);
        if let Some(existing) = &existing {
            if changed.is_empty()
                && deleted.is_empty()
                && !mode_changed
                && extractor_ok
                && existing.index_version == gry_store::FORMAT_VERSION
            {
                let (nodes, edges) = store.counts(&project)?;
                let report = IndexReport {
                    project: project.clone(),
                    root_path: root.to_string_lossy().to_string(),
                    mode: mode.as_str().to_string(),
                    generation: existing.generation.clone(),
                    files: discovery.files.len(),
                    source_bytes: discovery.source_bytes,
                    nodes,
                    edges,
                    parsed_files: 0,
                    reused_files: discovery.files.len(),
                    deleted_files: 0,
                    duration_ms: started.elapsed().as_millis(),
                    unchanged: true,
                    skipped: discovery
                        .coverage
                        .iter()
                        .filter(|c| c.kind != "parse_partial" && c.kind != "parse_unusable")
                        .map(|c| SkippedFile {
                            path: c.rel_path.clone(),
                            reason: c.detail.clone(),
                            phase: c.kind.clone(),
                        })
                        .collect(),
                    excluded: excluded_report(&discovery),
                    parse_partial: parse_partial_rows(&store.coverage(&project)?),
                    coverage: CoverageSummary {
                        gaps: store.coverage(&project)?.len(),
                        parse_partial: 0,
                        ignored_files: discovery.ignored_files.len(),
                        ignored_total: discovery.ignored_files.len(),
                        hash_records_complete: true,
                    },
                    artifact: None,
                    artifact_error: None,
                    cross_repo: None,
                };
                if request.persistence {
                    let artifact = Config::artifact_path(&root);
                    match store.export_artifact(&project, &artifact) {
                        Ok(_) => {
                            let mut report = report;
                            report.artifact = Some(artifact.to_string_lossy().to_string());
                            return Ok(report);
                        }
                        Err(e) => {
                            let mut report = report;
                            report.artifact_error = Some(e.to_string());
                            return Ok(report);
                        }
                    }
                }
                return Ok(report);
            }
        }

        if let Some(existing) = &existing {
            if timing_enabled() {
                eprintln!(
                    "[gry] delta check existing={} extractor_ok={} mode_changed={} version={}/{} changed={} deleted={}",
                    true, extractor_ok, mode_changed, existing.index_version, gry_store::FORMAT_VERSION,
                    changed.len(), deleted.len()
                );
            }
            if extractor_ok
                && !mode_changed
                && existing.index_version == gry_store::FORMAT_VERSION
                && (!changed.is_empty() || !deleted.is_empty())
            {
                let delta = crate::delta::try_incremental(crate::delta::DeltaContext {
                    config: &self.config,
                    store: &mut store,
                    project: &project,
                    root: &root,
                    mode,
                    discovery: &discovery,
                    changed: &changed,
                    deleted: &deleted,
                    known_files: &existing_files,
                    existing,
                    request,
                    started,
                })?;
                if let Some(report) = delta {
                    return Ok(report);
                }
            }
        }

        // A full rebuild allocates the whole graph; delta and unchanged paths
        // do not, so the budget is enforced only before a full build.
        if let Some(limit_mb) = self.config.max_index_memory_mb() {
            let estimate_mb = estimate_peak_mb(discovery.source_bytes, discovery.files.len());
            if estimate_mb > limit_mb {
                return Err(gry_core::GryError::invalid(format!(
                    "index memory budget exceeded: estimated {estimate_mb} MiB > {limit_mb} MiB \
                     (raise GRY_MAX_INDEX_MEMORY_MB / index_max_memory_mb, or narrow the repository)"
                )));
            }
        }

        let t = Instant::now();
        let cache = store.parse_cache(&project)?;
        phase!("parse-cache load", t);
        let cache_map: HashMap<String, (String, Vec<u8>)> = cache
            .into_iter()
            .map(|(rel, sha, data)| (rel, (sha, data)))
            .collect();

        // Reuse cached extraction only when hashes and extractor version match.
        let t = Instant::now();
        let mut fresh: HashMap<String, FileExtract> = HashMap::new();
        let mut parse_errors: Vec<(String, String)> = Vec::new();
        let mut to_parse: Vec<&DiscoveredFile> = Vec::new();
        let mut reused = 0usize;
        let mut reused_keys: HashMap<String, usize> = HashMap::new();
        let mut decode_jobs: Vec<(&DiscoveredFile, &[u8])> = Vec::new();
        if extractor_ok {
            for file in &discovery.files {
                match cache_map.get(&file.rel_path) {
                    Some((sha, data)) if *sha == file.sha256 => {
                        reused_keys.insert(file.rel_path.clone(), decode_jobs.len());
                        decode_jobs.push((file, data.as_slice()));
                    }
                    _ => to_parse.push(file),
                }
            }
        } else {
            to_parse.extend(discovery.files.iter());
        }
        let decoded: Vec<Option<FileExtract>> = decode_jobs
            .par_iter()
            .map(|(_, data)| decode_cache(data))
            .collect();
        for ((file, _), extract) in decode_jobs.into_iter().zip(decoded) {
            match extract {
                Some(extract) => {
                    fresh.insert(file.rel_path.clone(), extract);
                    reused += 1;
                }
                None => to_parse.push(file),
            }
        }

        let parsed_results: Vec<(String, Result<FileExtract, String>)> = to_parse
            .par_iter()
            .map(|file| {
                if cancel.load(Ordering::Relaxed) {
                    return (file.rel_path.clone(), Err("cancelled".to_string()));
                }
                match std::fs::read(&file.abs_path) {
                    Ok(bytes) => {
                        let lang = gry_core::language::detect(&file.rel_path)
                            .unwrap_or_else(|| gry_core::language::detect("x.txt").unwrap());
                        let extract = extract_file(&file.rel_path, lang, &bytes);
                        (file.rel_path.clone(), Ok(extract))
                    }
                    Err(e) => (file.rel_path.clone(), Err(e.to_string())),
                }
            })
            .collect();
        if cancel.load(Ordering::Relaxed) {
            return Err(gry_core::GryError::Cancelled);
        }
        for (rel, result) in parsed_results {
            match result {
                Ok(extract) => {
                    fresh.insert(rel, extract);
                }
                Err(e) => parse_errors.push((rel, e)),
            }
        }
        phase!("cache decode + parse", t);
        let parsed_files = to_parse.len().saturating_sub(parse_errors.len());
        let mut parsed: Vec<ParsedFile> = Vec::with_capacity(discovery.files.len());
        for file in &discovery.files {
            let extract = fresh.remove(&file.rel_path).unwrap_or_else(|| FileExtract {
                language: file.language.clone(),
                sha256: file.sha256.clone(),
                size: file.size as u64,
                ..Default::default()
            });
            parsed.push(ParsedFile {
                discovered: file.clone(),
                extract,
            });
        }
        parse_errors.sort_by(|a, b| a.0.cmp(&b.0));

        let branch = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD");
        let working_tree = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);
        let options = BuildOptions {
            project: project.clone(),
            root_path: root.to_string_lossy().to_string(),
            mode: mode.as_str().to_string(),
            semantic_edges: mode.semantic_edges(),
            branch,
            working_tree,
            partial: None,
            nodes_only: false,
        };
        let t = Instant::now();
        let mut result = build(&parsed, &options);
        phase!("graph build/resolve", t);
        let t = Instant::now();
        if mode == IndexMode::Full {
            crate::history::add_file_coupling(&mut result.builder);
        }
        phase!("git history coupling", t);
        if timing_enabled() {
            eprintln!(
                "[gry] full symbols={} parsed={}",
                result.symbols.len(),
                parsed.len()
            );
        }
        let (nodes, edges, vectors) = result.builder.finish();

        // Coverage: discovery gaps + parse gaps + unreadable files.
        let mut coverage: Vec<CoverageRow> = discovery
            .coverage
            .iter()
            .map(|c| CoverageRow {
                rel_path: c.rel_path.clone(),
                kind: c.kind.clone(),
                detail: c.detail.clone(),
            })
            .collect();
        for (rel, err) in &parse_errors {
            coverage.push(CoverageRow {
                rel_path: rel.clone(),
                kind: "read".to_string(),
                detail: err.clone(),
            });
        }
        let mut parse_partial_count = 0usize;
        for file in &parsed {
            if file.extract.parse_gaps.is_empty() {
                continue;
            }
            let ranges: Vec<String> = file
                .extract
                .parse_gaps
                .iter()
                .map(|g| format!("{}-{}", g.start_line, g.end_line))
                .collect();
            let unusable = file
                .extract
                .parse_gaps
                .iter()
                .any(|g| g.reason == "parse_unusable");
            coverage.push(CoverageRow {
                rel_path: file.discovered.rel_path.clone(),
                kind: if unusable {
                    "parse_unusable".to_string()
                } else {
                    "parse_partial".to_string()
                },
                detail: ranges.join(","),
            });
            parse_partial_count += 1;
        }

        let generation = gry_store::new_generation();
        let file_hashes: Vec<FileHash> = discovery
            .files
            .iter()
            .map(|f| FileHash {
                rel_path: f.rel_path.clone(),
                sha256: f.sha256.clone(),
                mtime_ns: f.mtime_ns,
                size: f.size,
            })
            .collect();
        let t = Instant::now();
        let changed_paths: HashSet<&str> = changed.iter().map(|f| f.rel_path.as_str()).collect();
        let parse_cache: Vec<(String, String, Vec<u8>)> = parsed
            .iter()
            .filter_map(|file| {
                let rel = &file.discovered.rel_path;
                if !changed_paths.contains(rel.as_str()) {
                    if let Some((sha, data)) = cache_map.get(rel) {
                        if *sha == file.discovered.sha256 && !data.is_empty() {
                            return Some((rel.clone(), sha.clone(), data.clone()));
                        }
                    }
                }
                let data = encode_cache(&file.extract)?;
                Some((rel.clone(), file.discovered.sha256.clone(), data))
            })
            .collect();
        phase!("parse-cache encode", t);
        let coverage_meta = CoverageMeta {
            generation: generation.clone(),
            index_mode: mode.as_str().to_string(),
            recorded_at: gry_store::now_iso(),
            recording_status: "complete".to_string(),
            ignored_files_stored: discovery.ignored_files.len() as i64,
            ignored_files_total: discovery.ignored_files.len() as i64,
            coverage_version: 1,
            hash_records_complete: true,
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(gry_core::GryError::Cancelled);
        }
        let root_string = root.to_string_lossy().to_string();
        let mut write = ProjectWrite::new(&project, &root_string, mode.as_str(), &generation);
        write.files = &file_hashes;
        write.nodes = &nodes;
        write.edges = &edges;
        let vectors: Vec<(String, Vec<i8>)> = vectors.into_iter().collect();
        write.vectors = &vectors;
        write.parse_cache = &parse_cache;
        write.coverage = &coverage;
        write.coverage_meta = Some(&coverage_meta);
        let symbol_rows: Vec<gry_store::SymbolRow> = result
            .symbols
            .iter()
            .map(crate::delta::symbol_to_row)
            .collect();
        let refs = crate::delta::refs_from_extracts(&parsed);
        let file_meta: Vec<gry_store::FileMetaRow> = parsed
            .iter()
            .map(|f| gry_store::FileMetaRow {
                rel_path: f.discovered.rel_path.clone(),
                language: f.discovered.language.clone(),
                module_is_dir: f.discovered.module_is_dir,
            })
            .collect();
        write.symbols = &symbol_rows;
        write.refs = &refs;
        write.file_meta = &file_meta;
        drop(parsed);
        drop(result.registry);
        let t = Instant::now();
        let stats = store.publish(&write)?;
        phase!("publish", t);
        store.set_meta("extractor_version", EXTRACTOR_VERSION)?;
        store.checkpoint()?;

        let mut artifact = None;
        let mut artifact_error = None;
        if request.persistence {
            let path = Config::artifact_path(&root);
            match store.export_artifact(&project, &path) {
                Ok(_) => artifact = Some(path.to_string_lossy().to_string()),
                Err(e) => artifact_error = Some(e.to_string()),
            }
        }

        let skipped = discovery
            .coverage
            .iter()
            .filter(|c| c.kind != "parse_partial" && c.kind != "parse_unusable")
            .map(|c| SkippedFile {
                path: c.rel_path.clone(),
                reason: c.detail.clone(),
                phase: c.kind.clone(),
            })
            .chain(parse_errors.iter().map(|(rel, err)| SkippedFile {
                path: rel.clone(),
                reason: err.clone(),
                phase: "read".to_string(),
            }))
            .collect();
        let report = IndexReport {
            project: project.clone(),
            root_path: root.to_string_lossy().to_string(),
            mode: mode.as_str().to_string(),
            generation,
            files: discovery.files.len(),
            source_bytes: discovery.source_bytes,
            nodes: stats.nodes,
            edges: stats.edges,
            parsed_files,
            reused_files: reused,
            deleted_files: deleted.len(),
            duration_ms: started.elapsed().as_millis(),
            unchanged: false,
            skipped,
            excluded: excluded_report(&discovery),
            parse_partial: parse_partial_rows(&coverage),
            coverage: CoverageSummary {
                gaps: coverage.len(),
                parse_partial: parse_partial_count,
                ignored_files: discovery.ignored_files.len(),
                ignored_total: discovery.ignored_files.len(),
                hash_records_complete: true,
            },
            artifact,
            artifact_error,
            cross_repo: None,
        };
        let _ = store.set_meta("last_index_report", &serde_json::to_string(&report)?);
        Ok(report)
    }
}

pub(crate) fn excluded_report(discovery: &crate::discover::Discovery) -> ExcludedReport {
    let count = discovery.excluded_dirs.len();
    ExcludedReport {
        dirs: discovery.excluded_dirs.iter().take(25).cloned().collect(),
        count,
        truncated: count > 25,
    }
}

pub(crate) fn parse_partial_rows(coverage: &[CoverageRow]) -> Vec<SkippedFile> {
    coverage
        .iter()
        .filter(|c| c.kind == "parse_partial" || c.kind == "parse_unusable")
        .map(|c| SkippedFile {
            path: c.rel_path.clone(),
            reason: c.detail.clone(),
            phase: c.kind.clone(),
        })
        .collect()
}

fn estimate_peak_mb(source_bytes: i64, files: usize) -> u64 {
    // Calibrated from the 341-file C benchmark: graph JSON, parse cache and
    // vectors dominate at roughly 40x source plus a fixed allocator baseline.
    let source_mb = (source_bytes.max(0) as u64) / (1024 * 1024);
    128 + source_mb.saturating_mul(40) + (files as u64) / 8
}

pub(crate) fn encode_cache(extract: &FileExtract) -> Option<Vec<u8>> {
    let bytes = rmp_serde::to_vec(extract).ok()?;
    zstd::encode_all(bytes.as_slice(), 3).ok()
}

pub(crate) fn decode_cache(data: &[u8]) -> Option<FileExtract> {
    let bytes = match zstd::decode_all(data) {
        Ok(bytes) => bytes,
        Err(e) => {
            if timing_enabled() {
                eprintln!("[gry] cache zstd error: {e}");
            }
            return None;
        }
    };
    match rmp_serde::from_slice(&bytes) {
        Ok(extract) => Some(extract),
        Err(e) => {
            if timing_enabled() {
                eprintln!("[gry] cache msgpack error: {e}");
            }
            None
        }
    }
}

pub struct ProjectLock {
    file: std::fs::File,
    path: PathBuf,
}

impl ProjectLock {
    pub fn acquire(config: &Config, project: &str) -> gry_core::GryResult<Self> {
        config.ensure_dirs()?;
        let dir = config.lock_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{project}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        use fs2::FileExt;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file, path }),
            Err(_) => Err(gry_core::GryError::Busy(format!(
                "another indexing run holds the lock for {project}"
            ))),
        }
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write job status atomically for detached CLI workers.
pub fn write_job_status(config: &Config, project: &str, status: &serde_json::Value) {
    let dir = config.job_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{project}.json"));
    let tmp = path.with_extension("json.tmp");
    if let Ok(mut file) = std::fs::File::create(&tmp) {
        let _ = file.write_all(status.to_string().as_bytes());
        let _ = std::fs::rename(tmp, path);
    }
}

pub fn read_job_status(config: &Config, project: &str) -> Option<serde_json::Value> {
    let path = config.job_dir().join(format!("{project}.json"));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}
