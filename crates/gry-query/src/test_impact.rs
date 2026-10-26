//! Conservative test-impact selection and team artifact publishing.
//!
//! The engine narrows only when it can prove the omitted tests cannot be
//! affected. Missing configuration, coverage, Git data or a reachable test
//! mapping all produce `run_all`.

use gry_core::types::{labels, Node};
use gry_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImpactConfig {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub tests: TestsConfig,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default = "default_unmapped")]
    pub unmapped_files: String,
    #[serde(default)]
    pub coverage: Option<CoverageConfig>,
}

fn default_unmapped() -> String {
    "run_all".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestsConfig {
    #[serde(default)]
    pub presets: Option<Value>,
    #[serde(default)]
    pub test_globs: Vec<String>,
    #[serde(default)]
    pub conventions: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub unreferenced: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoverageConfig {
    #[serde(default)]
    pub compatibility_paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    test_impact: Option<ImpactConfig>,
}

pub fn load_config(repo: &Path, override_path: Option<&str>) -> gry_core::GryResult<ImpactConfig> {
    let path = override_path
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { repo.join(p) })
        .unwrap_or_else(|| repo.join(".gry.json"));
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            return Err(gry_core::GryError::invalid(
                "test-impact config not found; every test runs (run_all)",
            ))
        }
    };
    let file: ConfigFile = serde_json::from_slice(&bytes)
        .map_err(|e| gry_core::GryError::invalid(format!("invalid test-impact config: {e}")))?;
    let mut config = file
        .test_impact
        .ok_or_else(|| gry_core::GryError::invalid("test_impact section is missing"))?;
    if config.tests.test_globs.is_empty()
        && config
            .tests
            .presets
            .as_ref()
            .and_then(|p| p.get("c"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        config.tests.test_globs = vec![
            "tests/**".to_string(),
            "test/**".to_string(),
            "**/*_test.c".to_string(),
            "**/*_test.cpp".to_string(),
            "**/*_test.cc".to_string(),
        ];
    }
    if config.version != 1 {
        return Err(gry_core::GryError::invalid(format!(
            "unsupported test_impact.version {} (expected 1)",
            config.version
        )));
    }
    Ok(config)
}

pub struct SelectRequest {
    pub repo: String,
    pub base: Option<String>,
    pub since: Option<String>,
    pub config_path: Option<String>,
    pub coverage_dir: Option<String>,
    pub platform: String,
}

pub fn select_tests(store: &Store, req: &SelectRequest) -> gry_core::GryResult<Value> {
    let repo = Path::new(&req.repo)
        .canonicalize()
        .map_err(|e| gry_core::GryError::invalid(format!("repo not found: {e}")))?;
    let project = store
        .project_by_root(&repo.to_string_lossy())?
        .ok_or_else(|| gry_core::GryError::ProjectNotFound(repo.to_string_lossy().to_string()))?;
    let changed = crate::impact::changed_paths(&repo, req.base.as_deref(), req.since.as_deref())
        .unwrap_or_default();
    if changed.is_empty() {
        return Ok(json!({
            "experimental": true,
            "run_all": true,
            "reason": "NO_CHANGES_OR_GIT_UNAVAILABLE",
            "changed_files": [],
            "suites": all_suites(store, &project.name)?,
            "tests": all_tests(store, &project.name)?,
        }));
    }
    let config = match load_config(&repo, req.config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            return Ok(json!({
                "experimental": true,
                "run_all": true,
                "reason": "CONFIG_UNAVAILABLE",
                "detail": e.to_string(),
                "changed_files": changed,
                "suites": all_suites(store, &project.name)?,
                "tests": all_tests(store, &project.name)?,
            }))
        }
    };
    let test_globs = compile_globs(&config.tests.test_globs);
    let nodes = store.all_nodes(&project.name)?;
    let edges = store.all_edges(&project.name)?;
    let by_id: HashMap<i64, &Node> = nodes.iter().map(|n| (n.id, n)).collect();
    let test_nodes: Vec<&Node> = nodes
        .iter()
        .filter(|n| {
            n.property_bool("is_test").unwrap_or(false)
                || is_test_path(&n.file_path)
                || test_globs
                    .as_ref()
                    .map(|g| g.is_match(&n.file_path))
                    .unwrap_or(false)
        })
        .collect();

    // Index TESTS edges and file references.
    let mut tests_by_target: HashMap<String, HashSet<String>> = HashMap::new();
    let mut test_file_refs: HashMap<String, HashSet<String>> = HashMap::new();
    for edge in &edges {
        let Some(source) = by_id.get(&edge.source_id) else {
            continue;
        };
        let Some(target) = by_id.get(&edge.target_id) else {
            continue;
        };
        if edge.edge_type == "TESTS" {
            tests_by_target
                .entry(target.qualified_name.clone())
                .or_default()
                .insert(source.qualified_name.clone());
        }
        if edge.edge_type == "REFERENCES_FILE" && source.property_bool("is_test").unwrap_or(false) {
            test_file_refs
                .entry(target.file_path.clone())
                .or_default()
                .insert(source.qualified_name.clone());
        }
    }

    let mut reasons: Vec<Value> = Vec::new();
    let mut selected: HashSet<String> = HashSet::new();
    let mut run_all = false;
    let mut segment_filters: HashSet<String> = HashSet::new();
    let mut uncovered_files: Vec<String> = Vec::new();
    for path in &changed {
        let rule = match_rule(&config, path);
        match rule {
            None => {
                if config.unmapped_files == "ignore" {
                    reasons.push(json!({"code": "UNMAPPED_IGNORED", "path": path}));
                    continue;
                }
                run_all = true;
                reasons.push(json!({"code": "UNMAPPED_FILE", "path": path}));
            }
            Some(rule) => match rule.action.as_str() {
                "ignore" => {
                    reasons.push(json!({"code": "IGNORED", "path": path, "rule": rule.id}));
                }
                "run_all" => {
                    run_all = true;
                    reasons.push(json!({"code": "RULE_RUN_ALL", "path": path, "rule": rule.id}));
                }
                "referencing_tests" => {
                    if let Some(segment) = rule
                        .key
                        .as_deref()
                        .and_then(|key| key.strip_prefix("path_segment:"))
                        .and_then(|n| n.parse::<usize>().ok())
                    {
                        if let Some(value) = path.split('/').nth(segment.saturating_sub(1)) {
                            if !value.is_empty() {
                                segment_filters.insert(value.to_string());
                            }
                        }
                    }
                    let changed_symbols: Vec<&Node> = nodes
                        .iter()
                        .filter(|n| n.file_path == *path)
                        .filter(|n| {
                            !matches!(n.label.as_str(), "File" | "Folder" | "Project" | "Package")
                        })
                        .collect();
                    if changed_symbols.is_empty() {
                        if rule.unreferenced.as_deref() == Some("ignore") {
                            reasons.push(json!({"code": "UNREFERENCED_IGNORED", "path": path, "rule": rule.id}));
                        } else {
                            run_all = true;
                            reasons.push(
                                json!({"code": "UNREFERENCED", "path": path, "rule": rule.id}),
                            );
                        }
                        continue;
                    }
                    let mut found = false;
                    for symbol in &changed_symbols {
                        if let Some(tests) = tests_by_target.get(&symbol.qualified_name) {
                            for test in tests {
                                selected.insert(test.clone());
                                found = true;
                            }
                        }
                    }
                    if let Some(tests) = test_file_refs.get(path) {
                        for test in tests {
                            selected.insert(test.clone());
                            found = true;
                        }
                    }
                    if !found {
                        if rule.unreferenced.as_deref() == Some("ignore") {
                            reasons.push(json!({"code": "UNREFERENCED_IGNORED", "path": path, "rule": rule.id}));
                        } else {
                            run_all = true;
                            reasons.push(
                                json!({"code": "UNREFERENCED", "path": path, "rule": rule.id}),
                            );
                        }
                    } else {
                        reasons.push(
                            json!({"code": "REFERENCING_TESTS", "path": path, "rule": rule.id}),
                        );
                    }
                }
                other => {
                    run_all = true;
                    reasons.push(json!({"code": "INVALID_ACTION", "path": path, "action": other}));
                }
            },
        }
    }

    // Coverage admission.
    let coverage = match &req.coverage_dir {
        Some(dir) => load_coverage(dir, &req.platform, &repo),
        None => Err("coverage map not provided".to_string()),
    };
    let coverage_admitted = coverage.as_ref().map(|c| c.admitted).unwrap_or(false);
    if coverage.is_err() {
        run_all = true;
        reasons.push(json!({"code": "COVERAGE_UNAVAILABLE", "detail": coverage.as_ref().err().cloned().unwrap_or_default()}));
    }
    if let Ok(coverage) = &coverage {
        // Any test that covers a changed function is selected.
        for path in &changed {
            for symbol in nodes.iter().filter(|n| n.file_path == *path) {
                if let Some(tests) = coverage
                    .tests_by_function
                    .get(&symbol.qualified_name)
                    .or_else(|| coverage.tests_by_function.get(&symbol.name))
                {
                    for test in tests {
                        selected.insert(test.clone());
                    }
                }
            }
        }
        if !coverage.admitted {
            run_all = true;
            reasons.push(json!({"code": "COVERAGE_NOT_ADMITTED", "detail": coverage.reason}));
        }
    }
    if !uncovered_files.is_empty() {
        uncovered_files.clear();
    }
    let mut selected_nodes: Vec<&Node> = test_nodes
        .iter()
        .copied()
        .filter(|n| selected.contains(&n.qualified_name))
        .collect();
    if !segment_filters.is_empty() {
        let before = selected_nodes.len();
        selected_nodes.retain(|n| {
            n.file_path
                .split('/')
                .any(|part| segment_filters.contains(part))
        });
        // If the segment rule would omit every candidate, fall back to run_all
        // rather than silently dropping coverage.
        if before > 0 && selected_nodes.is_empty() {
            run_all = true;
            reasons.push(json!({
                "code": "SEGMENT_FILTER_EMPTY",
                "segments": segment_filters.iter().cloned().collect::<Vec<_>>()
            }));
        }
    }
    let selected_tests: Vec<Value> = selected_nodes
        .iter()
        .map(|n| json!({"qn": n.qualified_name, "name": n.name, "file": n.file_path}))
        .collect();
    let suites = suites_for(store, &project.name, &selected)?;
    Ok(json!({
        "experimental": true,
        "project": project.name,
        "run_all": run_all,
        "changed_files": changed,
        "reasons": reasons,
        "selected_tests": selected_tests,
        "suites": if run_all { all_suites(store, &project.name)? } else { suites },
        "coverage_admitted": coverage_admitted,
        "coverage_path": req.coverage_dir,
        "note": "Never narrows without admitted coverage; unknown inputs run everything.",
    }))
}

fn match_rule<'a>(config: &'a ImpactConfig, path: &str) -> Option<&'a Rule> {
    for rule in &config.rules {
        for pattern in &rule.paths {
            if glob_match(pattern, path) {
                return Some(rule);
            }
        }
    }
    None
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let normalized = pattern.trim_start_matches('/');
    let glob = match globset::GlobBuilder::new(normalized)
        .literal_separator(false)
        .build()
    {
        Ok(g) => g.compile_matcher(),
        Err(_) => return false,
    };
    if glob.is_match(path) {
        return true;
    }
    if normalized.ends_with('/') {
        return glob.is_match(format!("{path}/"));
    }
    false
}

fn compile_globs(patterns: &[String]) -> Option<globset::GlobSet> {
    if patterns.is_empty() {
        return None;
    }
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        if let Ok(glob) = globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
        {
            builder.add(glob);
        }
    }
    builder.build().ok()
}

fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("__tests__")
        || p.ends_with("_test.c")
        || p.ends_with("_test.cpp")
        || p.ends_with("_test.go")
        || p.ends_with("_test.py")
        || p.ends_with("_test.rs")
        || p.ends_with(".test.ts")
        || p.ends_with(".spec.ts")
        || p.ends_with(".test.js")
}

fn all_tests(store: &Store, project: &str) -> gry_core::GryResult<Vec<Value>> {
    Ok(store
        .all_nodes(project)?
        .into_iter()
        .filter(|n| n.property_bool("is_test").unwrap_or(false) || is_test_path(&n.file_path))
        .filter(|n| labels::is_callable(&n.label))
        .map(|n| json!({"qn": n.qualified_name, "name": n.name, "file": n.file_path}))
        .collect())
}

fn all_suites(store: &Store, project: &str) -> gry_core::GryResult<Vec<Value>> {
    let mut suites: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in store.all_nodes(project)? {
        if (node.property_bool("is_test").unwrap_or(false) || is_test_path(&node.file_path))
            && labels::is_callable(&node.label)
        {
            suites
                .entry(node.file_path.clone())
                .or_default()
                .push(node.qualified_name);
        }
    }
    Ok(suites
        .into_iter()
        .map(|(file, tests)| json!({"suite": file, "whole": true, "tests": tests}))
        .collect())
}

fn suites_for(
    store: &Store,
    project: &str,
    selected: &HashSet<String>,
) -> gry_core::GryResult<Vec<Value>> {
    let mut suites: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in store.all_nodes(project)? {
        if selected.contains(&node.qualified_name) {
            suites
                .entry(node.file_path.clone())
                .or_default()
                .push(node.name.clone());
        }
    }
    Ok(suites
        .into_iter()
        .map(|(file, tests)| json!({"suite": file, "whole": false, "tests": tests}))
        .collect())
}

struct Coverage {
    admitted: bool,
    reason: String,
    tests_by_function: HashMap<String, Vec<String>>,
}

fn load_coverage(dir: &str, platform: &str, repo: &Path) -> Result<Coverage, String> {
    let dir = Path::new(dir);
    let meta_path = dir.join("meta.json");
    let bytes = fs::read(&meta_path).map_err(|e| format!("missing coverage meta.json: {e}"))?;
    let meta: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid coverage meta.json: {e}"))?;
    let commit = meta.get("commit").and_then(Value::as_str).unwrap_or("");
    let meta_platform = meta.get("platform").and_then(Value::as_str).unwrap_or("");
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let mut admitted = true;
    let mut reason = String::new();
    if commit != head {
        admitted = false;
        reason = format!("coverage commit {commit} != HEAD {head}");
    } else if meta_platform != platform {
        admitted = false;
        reason = format!("coverage platform {meta_platform} != {platform}");
    }
    // Parse tests.tsv: function<TAB>test<TAB>suite (or test<TAB>function).
    let tests_path = dir.join("tests.tsv");
    let mut tests_by_function: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(text) = fs::read_to_string(&tests_path) {
        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let function = parts[0].trim();
            let test = parts[1].trim();
            if !function.is_empty() && !test.is_empty() {
                tests_by_function
                    .entry(function.to_string())
                    .or_default()
                    .push(test.to_string());
            }
        }
    }
    Ok(Coverage {
        admitted,
        reason,
        tests_by_function,
    })
}

pub struct PublishRequest {
    pub repo: String,
    pub out: String,
    pub coverage_dir: Option<String>,
    pub platform: String,
}

/// Publish a frozen graph + coverage + receipt bundle.
pub fn publish(store: &mut Store, req: &PublishRequest) -> gry_core::GryResult<Value> {
    let repo = Path::new(&req.repo)
        .canonicalize()
        .map_err(|e| gry_core::GryError::invalid(format!("repo not found: {e}")))?;
    let project = store
        .project_by_root(&repo.to_string_lossy())?
        .ok_or_else(|| gry_core::GryError::ProjectNotFound(repo.to_string_lossy().to_string()))?;
    let out = PathBuf::from(&req.out);
    fs::create_dir_all(&out)?;
    let graph_path = out.join("graph.db.zst");
    store.export_artifact(&project.name, &graph_path)?;
    let mut coverage_sha = None;
    if let Some(dir) = &req.coverage_dir {
        let src = Path::new(dir);
        for name in ["functions.tsv", "tests.tsv", "meta.json"] {
            let from = src.join(name);
            if from.is_file() {
                let to = out.join(name);
                if from != to {
                    fs::copy(&from, &to)?;
                }
            }
        }
        if let Ok(bytes) = fs::read(src.join("tests.tsv")) {
            coverage_sha = Some(gry_store::sha256_hex(&bytes));
        }
    }
    let graph_sha = fs::read(&graph_path)
        .map(|b| gry_store::sha256_hex(&b))
        .unwrap_or_default();
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let receipt = json!({
        "schema": 1,
        "project": project.name,
        "commit": commit,
        "platform": req.platform,
        "graph_sha256": graph_sha,
        "coverage_sha256": coverage_sha,
        "created_at": gry_store::now_iso(),
        "project_generation": project.generation,
    });
    fs::write(
        out.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    #[test]
    fn config_validation() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        std::fs::write(
            repo.join(".gry.json"),
            r#"{"test_impact":{"version":1,"tests":{"presets":{"c":true}},"rules":[{"id":"all","paths":["src/**"],"action":"run_all"}],"unmapped_files":"run_all"}}"#,
        )
        .unwrap();
        let config = load_config(repo, None).unwrap();
        assert_eq!(config.version, 1);
        assert_eq!(config.rules.len(), 1);
        assert!(match_rule(&config, "src/a.c").is_some());
        assert!(match_rule(&config, "docs/a.md").is_none());
    }

    #[test]
    fn unmatched_config_is_conservative() {
        let store = test_store();
        let out = select_tests(
            &store,
            &SelectRequest {
                repo: "/nonexistent".into(),
                base: None,
                since: None,
                config_path: None,
                coverage_dir: None,
                platform: "linux-x86_64".into(),
            },
        );
        assert!(out.is_err());
    }
}
