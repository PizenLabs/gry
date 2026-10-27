//! Thin agent lifecycle adapter over the bounded context engine.
//!
//! gry does not own an agent runtime. This module maps the four logical
//! lifecycle stages to context queries and returns a stable envelope that an
//! agent adapter (for example OHGO) can consume. Each stage is optional and
//! independently bounded. The adapter never mutates the repository and never
//! grants permissions.

use crate::context::{
    get_task_context, verify_freshness, ContextRequest, CONTEXT_CONTRACT_VERSION,
};
use gry_store::Store;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HOOK_STAGES: &[&str] = &["before_plan", "before_edit", "after_edit", "before_finish"];

#[derive(Debug, Clone)]
pub struct HookRequest {
    pub stage: String,
    pub project: String,
    pub task: Option<String>,
    pub targets: Vec<String>,
    pub changed_paths: Vec<String>,
    pub task_id: Option<String>,
    pub max_tokens: usize,
    pub timeout_ms: u64,
    pub cache: bool,
    pub cache_ttl_secs: u64,
}

impl Default for HookRequest {
    fn default() -> Self {
        Self {
            stage: "before_plan".to_string(),
            project: String::new(),
            task: None,
            targets: Vec::new(),
            changed_paths: Vec::new(),
            task_id: None,
            max_tokens: 3200,
            timeout_ms: 3000,
            cache: true,
            cache_ttl_secs: 30,
        }
    }
}

/// Run one lifecycle hook. `cache_dir` is optional: MCP callers pass `None` and
/// always receive a fresh result; the CLI adapter uses a short-lived on-disk
/// cache to suppress duplicate dispatch of the same stage for the same
/// generation and inputs.
pub fn run_hook(
    store: &Store,
    req: &HookRequest,
    cache_dir: Option<&Path>,
) -> gry_core::GryResult<Value> {
    let stage = req.stage.trim().to_ascii_lowercase();
    if !HOOK_STAGES.contains(&stage.as_str()) {
        return Err(gry_core::GryError::invalid(format!(
            "stage must be one of: {}",
            HOOK_STAGES.join(", ")
        )));
    }
    if hook_guard_active() {
        return Ok(unavailable_payload(
            &stage,
            req.task_id.as_deref(),
            "recursive_hook",
            "A gry hook is already running in this process tree; refusing recursive dispatch.",
        ));
    }
    run_hook_inner(store, req, cache_dir)
}

/// Run a hook after the caller has already checked the recursion guard. The
/// CLI uses this so it can set `GRY_HOOK_ACTIVE` for any process it spawns
/// without disabling its own hook execution.
#[doc(hidden)]
pub fn run_hook_after_guard_check(
    store: &Store,
    req: &HookRequest,
    cache_dir: Option<&Path>,
) -> gry_core::GryResult<Value> {
    run_hook_inner(store, req, cache_dir)
}

fn run_hook_inner(
    store: &Store,
    req: &HookRequest,
    cache_dir: Option<&Path>,
) -> gry_core::GryResult<Value> {
    let stage = req.stage.trim().to_ascii_lowercase();
    if !HOOK_STAGES.contains(&stage.as_str()) {
        return Err(gry_core::GryError::invalid(format!(
            "stage must be one of: {}",
            HOOK_STAGES.join(", ")
        )));
    }
    let started = std::time::Instant::now();
    let task_id = req
        .task_id
        .clone()
        .unwrap_or_else(|| default_task_id(&stage, req));
    let record = match store.require_project(&req.project) {
        Ok(record) => record,
        Err(gry_core::GryError::ProjectNotFound(_)) => {
            return Ok(json!({
                "hook": {
                    "stage": stage,
                    "task_id": task_id,
                    "status": "not_indexed",
                    "duration_ms": started.elapsed().as_millis(),
                    "cached": false,
                    "timeout_ms": req.timeout_ms,
                },
                "context": Value::Null,
                "failure": {
                    "code": "not_indexed",
                    "message": format!("project not found or not indexed: {}", req.project),
                    "retryable": true,
                }
            }));
        }
        Err(error) => return Err(error),
    };

    let key = hook_cache_key(&req.project, &record.generation, &stage, req);
    if req.cache && cache_inputs_are_fresh(store, req) {
        if let Some(cache_dir) = cache_dir {
            if let Some(mut cached) = read_cache(cache_dir, &key, &record.generation, req) {
                if payload_paths_are_fresh(store, &req.project, &cached) {
                    cached["hook"]["cached"] = json!(true);
                    cached["hook"]["duration_ms"] = json!(started.elapsed().as_millis());
                    return Ok(cached);
                }
            }
        }
    }

    let context_request = context_request_for(&stage, req);
    let context = match get_task_context(store, &context_request) {
        Ok(context) => context,
        Err(gry_core::GryError::ProjectNotFound(_)) => {
            return Ok(json!({
                "hook": {
                    "stage": stage,
                    "task_id": task_id,
                    "status": "not_indexed",
                    "duration_ms": started.elapsed().as_millis(),
                    "cached": false,
                    "timeout_ms": req.timeout_ms,
                },
                "context": Value::Null,
                "failure": {
                    "code": "not_indexed",
                    "message": format!("project not found or not indexed: {}", req.project),
                    "retryable": true,
                }
            }));
        }
        Err(error) => return Err(error),
    };
    let status = context["status"].as_str().unwrap_or("ok");
    let failure = if status == "ok" {
        Value::Null
    } else {
        json!({
            "code": status,
            "message": failure_message(status),
            "retryable": matches!(status, "timeout" | "not_found" | "partial" | "stale"),
        })
    };
    let payload = json!({
        "hook": {
            "stage": stage,
            "task_id": task_id,
            "status": status,
            "duration_ms": started.elapsed().as_millis(),
            "cached": false,
            "timeout_ms": req.timeout_ms,
        },
        "context": context,
        "failure": failure,
    });

    if req.cache && matches!(status, "ok" | "partial") && !context_is_stale(&context) {
        if let Some(cache_dir) = cache_dir {
            let _ = write_cache(cache_dir, &key, &record.generation, &payload);
        }
    }
    Ok(payload)
}

fn context_is_stale(context: &Value) -> bool {
    context["freshness"]["stale"].as_bool().unwrap_or(false)
        || context["evidence"]["freshness"].as_str() == Some("stale")
        || context["evidence"]["freshness"].as_str() == Some("generation_changed")
}

pub fn hook_guard_active() -> bool {
    std::env::var_os("GRY_HOOK_ACTIVE").is_some()
}

pub fn context_request_for(stage: &str, req: &HookRequest) -> ContextRequest {
    let (profile, depth, includes, max_nodes) = match stage {
        "before_plan" => (
            "scout",
            3,
            vec!["callers", "callees", "imports", "services", "coverage"],
            40,
        ),
        "before_edit" => (
            "context",
            2,
            vec![
                "callers", "callees", "imports", "services", "tests", "coverage",
            ],
            50,
        ),
        "after_edit" => (
            "auditor",
            2,
            vec![
                "callers", "callees", "imports", "services", "tests", "changes", "coverage",
            ],
            60,
        ),
        _ => (
            "auditor",
            2,
            vec![
                "callers", "callees", "imports", "services", "tests", "changes", "coverage",
            ],
            50,
        ),
    };
    ContextRequest {
        project: req.project.clone(),
        task: req.task.clone(),
        targets: req.targets.clone(),
        profile: profile.to_string(),
        depth,
        max_nodes,
        max_tokens: req.max_tokens,
        include: includes.into_iter().map(str::to_string).collect(),
        expected_generation: None,
        changed_paths: req.changed_paths.clone(),
        timeout_ms: req.timeout_ms,
        task_id: Some(
            req.task_id
                .clone()
                .unwrap_or_else(|| default_task_id(stage, req)),
        ),
        stage: Some(stage.to_string()),
        include_source: false,
    }
}

fn default_task_id(stage: &str, req: &HookRequest) -> String {
    let mut parts = vec![stage.to_string(), req.project.clone()];
    if let Some(task) = &req.task {
        parts.push(task.clone());
    }
    let mut targets = req.targets.clone();
    targets.sort();
    parts.extend(targets);
    let mut changed = req.changed_paths.clone();
    changed.sort();
    parts.extend(changed);
    let hash = gry_store::sha256_hex(parts.join("\u{1f}").as_bytes());
    format!("task-{}", &hash[..12])
}

fn failure_message(status: &str) -> &'static str {
    match status {
        "timeout" => "Context assembly exceeded its time budget; partial evidence is preserved.",
        "stale" => "The index is stale or the expected generation does not match; re-index before trusting the result as current.",
        "not_found" => "No target resolved. This is a resolution failure, not a verified empty result.",
        "partial" => "Some optional lookups failed; valid partial evidence is preserved.",
        "empty" => "No targets or task inputs produced context; this is an empty result, not an error.",
        _ => "Context is unavailable.",
    }
}

fn unavailable_payload(stage: &str, task_id: Option<&str>, code: &str, message: &str) -> Value {
    json!({
        "hook": {
            "stage": stage,
            "task_id": task_id,
            "status": "unavailable",
            "duration_ms": 0,
            "cached": false,
            "timeout_ms": 0,
        },
        "context": Value::Null,
        "failure": {
            "code": code,
            "message": message,
            "retryable": false,
        }
    })
}

/// A cache entry is only reused when the indexed inputs it was built from are
/// still current. A generation change invalidates through the key; a file edit
/// without re-indexing invalidates through mtime/size.
fn cache_inputs_are_fresh(store: &Store, req: &HookRequest) -> bool {
    let Ok(record) = store.require_project(&req.project) else {
        return false;
    };
    let mut paths: Vec<String> = req
        .changed_paths
        .iter()
        .map(|p| normalize_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    for target in &req.targets {
        let trimmed = target.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.contains('/') {
            paths.push(normalize_path(trimmed));
        } else if let Ok(Some(node)) = store.resolve_node(&req.project, trimmed) {
            if !node.file_path.is_empty() {
                paths.push(node.file_path);
            }
        } else if looks_like_path(trimmed) {
            paths.push(normalize_path(trimmed));
        }
    }
    paths.sort();
    paths.dedup();
    paths_are_fresh(store, &req.project, &record.root_path, &paths)
}

/// Validate every file referenced by a cached context, not only the request
/// inputs. A caller file can change without changing the target file.
fn payload_paths_are_fresh(store: &Store, project: &str, payload: &Value) -> bool {
    let Ok(record) = store.require_project(project) else {
        return false;
    };
    let mut paths = Vec::new();
    let context = payload.get("context").unwrap_or(payload);
    for key in ["nodes", "files", "services", "tests"] {
        if let Some(items) = context.get(key).and_then(Value::as_array) {
            for item in items {
                let path = item
                    .get("file")
                    .or_else(|| item.get("path"))
                    .and_then(Value::as_str);
                if let Some(path) = path.filter(|p| !p.is_empty()) {
                    paths.push(path.to_string());
                }
            }
        }
    }
    if let Some(rows) = context
        .get("changes")
        .and_then(|c| c.get("rows"))
        .and_then(Value::as_array)
    {
        for row in rows {
            if let Some(path) = row.get("path").and_then(Value::as_str) {
                paths.push(path.to_string());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths.truncate(128);
    paths_are_fresh(store, project, &record.root_path, &paths)
}

fn paths_are_fresh(store: &Store, project: &str, root_path: &str, paths: &[String]) -> bool {
    if paths.is_empty() {
        return true;
    }
    let Ok(record) = store.require_project(project) else {
        return false;
    };
    let set: BTreeSet<String> = paths.iter().cloned().collect();
    let check = verify_freshness(
        store,
        project,
        Path::new(root_path),
        &set,
        &record.generation,
    );
    !check.stale() && check.unverified == 0
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/') || value.contains('\\') || Path::new(value).extension().is_some()
}

fn cache_root(cache_dir: &Path) -> PathBuf {
    cache_dir.join("hooks")
}

fn hook_cache_key(project: &str, generation: &str, stage: &str, req: &HookRequest) -> String {
    let mut targets = req.targets.clone();
    targets.sort();
    let mut changed = req.changed_paths.clone();
    changed.sort();
    let canonical = json!({
        "contract": CONTEXT_CONTRACT_VERSION,
        "project": project,
        "generation": generation,
        "stage": stage,
        "task": req.task,
        "targets": targets,
        "changed": changed,
        "task_id": req.task_id,
        "max_tokens": req.max_tokens,
        "timeout_ms": req.timeout_ms,
    });
    gry_store::sha256_hex(canonical.to_string().as_bytes())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn read_cache(cache_dir: &Path, key: &str, generation: &str, req: &HookRequest) -> Option<Value> {
    let path = cache_root(cache_dir).join(format!("{key}.json"));
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 2_000_000 {
        return None;
    }
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    if value["contract"].as_u64() != Some(CONTEXT_CONTRACT_VERSION as u64) {
        return None;
    }
    if value["generation"].as_str() != Some(generation) {
        return None;
    }
    let created = value["created_at"].as_u64()?;
    if now_unix().saturating_sub(created) > req.cache_ttl_secs.min(3600) {
        return None;
    }
    value.get("payload").cloned()
}

fn write_cache(
    cache_dir: &Path,
    key: &str,
    generation: &str,
    payload: &Value,
) -> std::io::Result<()> {
    let dir = cache_root(cache_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{key}.json"));
    let tmp = dir.join(format!(".{key}.tmp"));
    let value = json!({
        "created_at": now_unix(),
        "contract": CONTEXT_CONTRACT_VERSION,
        "generation": generation,
        "payload": payload,
    });
    std::fs::write(&tmp, serde_json::to_vec(&value)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_core::types::{FileHash, NewNode};
    use gry_store::{EdgeSpec, ProjectWrite};

    fn store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let nodes = vec![
            NewNode::new("Function", "main", "p.src.main.main", "src/main.rs", 1, 10),
            NewNode::new(
                "Function",
                "handle",
                "p.src.lib.handle",
                "src/lib.rs",
                1,
                10,
            ),
        ];
        let edges = vec![EdgeSpec::new(
            "p.src.main.main",
            "p.src.lib.handle",
            "CALLS",
        )];
        let files = vec![FileHash {
            rel_path: "src/lib.rs".into(),
            sha256: "a".into(),
            mtime_ns: 1,
            size: 10,
        }];
        let mut w = ProjectWrite::new("p", "/tmp/p", "full", "gen-1");
        w.nodes = &nodes;
        w.edges = &edges;
        w.files = &files;
        store.publish(&w).unwrap();
        store
    }

    fn store_with_root(root: &Path) -> Store {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn handle() {}\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let nodes = vec![
            NewNode::new("Function", "main", "p.src.main.main", "src/main.rs", 1, 10),
            NewNode::new(
                "Function",
                "handle",
                "p.src.lib.handle",
                "src/lib.rs",
                1,
                10,
            ),
        ];
        let edges = vec![EdgeSpec::new(
            "p.src.main.main",
            "p.src.lib.handle",
            "CALLS",
        )];
        let mut files = Vec::new();
        for rel in ["src/lib.rs", "src/main.rs"] {
            let metadata = std::fs::metadata(root.join(rel)).unwrap();
            let bytes = std::fs::read(root.join(rel)).unwrap();
            let mtime_ns = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            files.push(FileHash {
                rel_path: rel.to_string(),
                sha256: gry_index::discover::sha256(&bytes),
                mtime_ns,
                size: metadata.len() as i64,
            });
        }
        let root_path = root.to_string_lossy().to_string();
        let mut w = ProjectWrite::new("p", &root_path, "full", "gen-1");
        w.nodes = &nodes;
        w.edges = &edges;
        w.files = &files;
        store.publish(&w).unwrap();
        store
    }

    fn req(stage: &str) -> HookRequest {
        HookRequest {
            stage: stage.into(),
            project: "p".into(),
            targets: vec!["handle".into()],
            ..Default::default()
        }
    }

    #[test]
    fn every_stage_maps_to_a_bounded_profile() {
        let store = store();
        for stage in HOOK_STAGES {
            let out = run_hook(&store, &req(stage), None).unwrap();
            assert_eq!(out["hook"]["stage"], *stage);
            assert!(out["context"]["budget"]["max_tokens"].as_u64().unwrap() >= 128);
            if *stage == "before_plan" {
                assert_eq!(out["context"]["profile"], "scout");
            }
            if *stage == "after_edit" || *stage == "before_finish" {
                assert_eq!(out["context"]["profile"], "auditor");
            }
        }
    }

    #[test]
    fn unknown_stage_is_invalid() {
        let store = store();
        assert!(run_hook(&store, &req("after_lunch"), None).is_err());
    }

    #[test]
    fn missing_project_is_not_an_empty_context() {
        let store = store();
        let mut request = req("before_edit");
        request.project = "missing".into();
        let out = run_hook(&store, &request, None).unwrap();
        assert_eq!(out["hook"]["status"], "not_indexed");
        assert_eq!(out["failure"]["code"], "not_indexed");
        assert!(out["context"].is_null());
    }

    #[test]
    fn cache_is_keyed_by_generation_and_inputs() {
        let base = HookRequest {
            project: "p".into(),
            targets: vec!["handle".into()],
            ..Default::default()
        };
        let a = hook_cache_key("p", "gen-1", "before_edit", &base);
        let b = hook_cache_key("p", "gen-2", "before_edit", &base);
        assert_ne!(a, b);
        let mut other = base.clone();
        other.targets = vec!["main".into()];
        assert_ne!(a, hook_cache_key("p", "gen-1", "before_edit", &other));
        assert_ne!(a, hook_cache_key("p", "gen-1", "after_edit", &base));
    }

    #[test]
    fn duplicate_dispatch_is_suppressed_and_marked_cached() {
        let root = tempfile::tempdir().unwrap();
        let store = store_with_root(root.path());
        let dir = tempfile::tempdir().unwrap();
        let first = run_hook(&store, &req("before_edit"), Some(dir.path())).unwrap();
        assert_eq!(first["hook"]["cached"], false);
        let second = run_hook(&store, &req("before_edit"), Some(dir.path())).unwrap();
        assert_eq!(second["hook"]["cached"], true);
        assert_eq!(
            first["context"]["generation"],
            second["context"]["generation"]
        );
    }

    #[test]
    fn cache_is_invalidated_by_an_unindexed_edit() {
        let root = tempfile::tempdir().unwrap();
        let store = store_with_root(root.path());
        let dir = tempfile::tempdir().unwrap();
        let first = run_hook(&store, &req("before_edit"), Some(dir.path())).unwrap();
        assert_eq!(first["hook"]["cached"], false);
        std::fs::write(
            root.path().join("src/lib.rs"),
            "pub fn handle() { let _ = 1; }\n",
        )
        .unwrap();
        let second = run_hook(&store, &req("before_edit"), Some(dir.path())).unwrap();
        assert_eq!(second["hook"]["cached"], false);
        assert_eq!(second["context"]["status"], "stale");
    }

    #[test]
    fn cache_is_invalidated_by_a_new_file_beside_the_evidence() {
        let root = tempfile::tempdir().unwrap();
        let store = store_with_root(root.path());
        let dir = tempfile::tempdir().unwrap();
        let first = run_hook(&store, &req("before_edit"), Some(dir.path())).unwrap();
        assert_eq!(first["hook"]["cached"], false);
        std::fs::write(root.path().join("src/added.rs"), "pub fn added() {}\n").unwrap();
        let second = run_hook(&store, &req("before_edit"), Some(dir.path())).unwrap();
        assert_eq!(second["hook"]["cached"], false);
        assert_eq!(second["context"]["status"], "stale");
        assert!(second["context"]["freshness"]["added_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/added.rs"));
    }

    #[test]
    fn cache_can_be_disabled() {
        let root = tempfile::tempdir().unwrap();
        let store = store_with_root(root.path());
        let dir = tempfile::tempdir().unwrap();
        let mut request = req("before_edit");
        request.cache = false;
        run_hook(&store, &request, Some(dir.path())).unwrap();
        let again = run_hook(&store, &request, Some(dir.path())).unwrap();
        assert_eq!(again["hook"]["cached"], false);
    }

    #[test]
    fn unavailable_payload_is_explicit() {
        let out = unavailable_payload("before_plan", Some("t"), "recursive_hook", "no");
        assert_eq!(out["hook"]["status"], "unavailable");
        assert_eq!(out["failure"]["retryable"], false);
    }

    #[test]
    fn after_edit_without_targets_uses_changed_paths() {
        let store = store();
        let mut request = req("after_edit");
        request.targets.clear();
        request.changed_paths = vec!["src/lib.rs".into()];
        let out = run_hook(&store, &request, None).unwrap();
        assert_eq!(out["context"]["changes"]["source"], "caller");
        assert_eq!(out["context"]["changes"]["total"], 1);
    }
}
