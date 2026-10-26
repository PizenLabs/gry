//! Change impact analysis: git diff → changed symbols → blast radius → risk.

use gry_core::types::{labels, Node};
use gry_store::traverse::{Direction, TraceParams};
use gry_store::Store;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DetectChangesRequest {
    pub project: String,
    pub scope: String,
    pub direction: String,
    pub depth: usize,
    pub edge_types: Vec<String>,
    pub fixpoint: bool,
    pub via: bool,
    pub seeds: bool,
    pub seed_filter: String,
    pub base_branch: Option<String>,
    pub since: Option<String>,
    pub limit: usize,
    pub impact_offset: usize,
    pub changed_limit: usize,
    pub changed_offset: usize,
    pub module_limit: usize,
    pub module_offset: usize,
    pub seed_limit: usize,
    pub seed_offset: usize,
    pub config_path: Option<String>,
}

impl Default for DetectChangesRequest {
    fn default() -> Self {
        Self {
            project: String::new(),
            scope: "impact".to_string(),
            direction: "inbound".to_string(),
            depth: 2,
            edge_types: Vec::new(),
            fixpoint: false,
            via: false,
            seeds: false,
            seed_filter: "all".to_string(),
            base_branch: None,
            since: None,
            limit: 200,
            impact_offset: 0,
            changed_limit: 20,
            changed_offset: 0,
            module_limit: 20,
            module_offset: 0,
            seed_limit: 50,
            seed_offset: 0,
            config_path: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ChangedFile {
    path: String,
    old_path: Option<String>,
    status: String,
    hunks: Vec<(i64, i64)>,
}

pub fn detect_changes(store: &Store, req: &DetectChangesRequest) -> gry_core::GryResult<Value> {
    let project = store.require_project(&req.project)?;
    let root = Path::new(&project.root_path);
    let base = req.base_branch.clone().or_else(|| detect_base_branch(root));
    let changed = changed_files(root, base.as_deref(), req.since.as_deref())?;
    let changed_total = changed.len();

    let mut changed_rows = Vec::new();
    let mut seeds: Vec<Node> = Vec::new();
    let mut seed_keys = HashSet::new();
    for file in &changed {
        let path = &file.path;
        // Deleted files keep their last indexed nodes so callers/tests can still
        // be reported until the graph is reindexed.
        let nodes = store.nodes_by_file(&req.project, path)?;
        let overlapping: Vec<&Node> = if file.hunks.is_empty() {
            nodes.iter().collect()
        } else {
            nodes
                .iter()
                .filter(|n| {
                    file.hunks.iter().any(|(start, end)| {
                        (n.start_line as i64) <= *end && (n.end_line as i64) >= *start
                    })
                })
                .collect()
        };
        let symbols: Vec<Value> = overlapping
            .iter()
            .filter(|n| !matches!(n.label.as_str(), "File" | "Folder" | "Project" | "Package"))
            .map(|n| {
                json!({
                    "qn": n.qualified_name,
                    "name": n.name,
                    "label": n.label,
                    "lines": [n.start_line, n.end_line],
                })
            })
            .collect();
        for node in overlapping {
            if matches!(
                node.label.as_str(),
                "File" | "Folder" | "Project" | "Package"
            ) {
                continue;
            }
            if seed_keys.insert(node.qualified_name.clone()) {
                seeds.push(node.clone());
            }
        }
        changed_rows.push(json!({
            "path": path,
            "status": file.status,
            "old_path": file.old_path,
            "hunks": file.hunks.iter().map(|(s,e)| [s,e]).collect::<Vec<_>>(),
            "symbols": symbols.iter().take(50).collect::<Vec<_>>(),
            "symbol_count": symbols.len(),
        }));
    }
    seeds.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    if req.seed_filter == "product" {
        seeds.retain(|n| {
            !n.property_bool("is_test").unwrap_or(false) && !is_test_path(&n.file_path)
        });
    }

    let mut impact: BTreeMap<String, ImpactRow> = BTreeMap::new();
    if req.scope != "files" {
        let edge_types = if req.edge_types.is_empty() {
            vec!["CALLS".to_string()]
        } else {
            req.edge_types.clone()
        };
        let direction = if req.fixpoint {
            Direction::Both
        } else {
            Direction::parse(&req.direction).unwrap_or(Direction::Inbound)
        };
        for seed in &seeds {
            let params = TraceParams {
                project: req.project.clone(),
                start_qn: seed.qualified_name.clone(),
                direction,
                depth: if req.fixpoint {
                    15
                } else {
                    req.depth.clamp(1, 15)
                },
                edge_types: edge_types.clone(),
                include_tests: req.scope == "tests",
                limit: 5000,
                parameter_name: None,
            };
            if let Some(trace) = store.trace(&params)? {
                for hop in trace.hops {
                    let entry = impact
                        .entry(hop.node.qualified_name.clone())
                        .or_insert_with(|| ImpactRow {
                            node: hop.node.clone(),
                            hop: hop.hop,
                            via_qn: hop.via_qn.clone(),
                            via_edge: hop.via_edge.clone(),
                            seed: seed.qualified_name.clone(),
                        });
                    if hop.hop < entry.hop {
                        entry.hop = hop.hop;
                        entry.via_qn = hop.via_qn.clone();
                        entry.via_edge = hop.via_edge.clone();
                        entry.seed = seed.qualified_name.clone();
                    }
                }
            }
        }
    }
    let mut impact_rows: Vec<ImpactRow> = impact.into_values().collect();
    impact_rows.sort_by(|a, b| {
        a.hop
            .cmp(&b.hop)
            .then(a.node.qualified_name.cmp(&b.node.qualified_name))
    });

    let (in_map, _) = degree_maps(store, &req.project)?;
    let seed_files: HashSet<String> = seeds.iter().map(|n| n.file_path.clone()).collect();
    let risk_for = |row: &ImpactRow| -> (&'static str, Vec<&'static str>) {
        let mut reasons = Vec::new();
        let incoming = in_map.get(&row.node.id).copied().unwrap_or(0);
        if row.node.property_bool("is_entry_point").unwrap_or(false) {
            reasons.push("entry_point");
        }
        if incoming >= 10 {
            reasons.push("high_fan_in");
        }
        if !seed_files.is_empty()
            && !seed_files
                .iter()
                .any(|f| top_dir(f) == top_dir(&row.node.file_path))
        {
            reasons.push("cross_component");
        }
        if labels::is_infrastructure(&row.node.label) {
            reasons.push("infrastructure_boundary");
        }
        let risk = if reasons.iter().any(|r| {
            matches!(
                *r,
                "entry_point" | "high_fan_in" | "infrastructure_boundary"
            )
        }) {
            "high"
        } else if !reasons.is_empty() {
            "medium"
        } else {
            "low"
        };
        (risk, reasons)
    };
    let mut risk_counts = json!({"high": 0, "medium": 0, "low": 0});
    let impact_json: Vec<Value> = impact_rows
        .iter()
        .skip(req.impact_offset)
        .take(req.limit.max(1))
        .map(|row| {
            let (risk, reasons) = risk_for(row);
            if let Some(v) = risk_counts.get_mut(risk) {
                *v = json!(v.as_i64().unwrap_or(0) + 1);
            }
            let mut value = json!({
                "qn": row.node.qualified_name,
                "name": row.node.name,
                "label": row.node.label,
                "file": row.node.file_path,
                "lines": [row.node.start_line, row.node.end_line],
                "hop": row.hop,
                "risk": risk,
                "reasons": reasons,
                "seed": row.seed,
            });
            if req.via {
                value["via_qn"] = json!(row.via_qn);
                value["via_edge"] = json!(row.via_edge);
            }
            value
        })
        .collect();

    let seed_rows: Vec<Value> = seeds
        .iter()
        .skip(req.seed_offset)
        .take(req.seed_limit.max(1))
        .map(|n| {
            json!({
                "qn": n.qualified_name,
                "name": n.name,
                "label": n.label,
                "file": n.file_path,
                "lines": [n.start_line, n.end_line],
                "test": n.property_bool("is_test").unwrap_or(false) || is_test_path(&n.file_path),
            })
        })
        .collect();

    let mut modules: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for file in &changed {
        let dir = top_dir(&file.path);
        modules.entry(dir).or_insert((0, 0)).0 += 1;
    }
    for row in &impact_rows {
        let dir = top_dir(&row.node.file_path);
        modules.entry(dir).or_insert((0, 0)).1 += 1;
    }
    let module_rows: Vec<Value> = modules
        .into_iter()
        .skip(req.module_offset)
        .take(req.module_limit.max(1))
        .map(|(module, (changed, impacted))| json!({"module": module, "changed_files": changed, "impacted_symbols": impacted}))
        .collect();

    let tests: Vec<Value> = impact_rows
        .iter()
        .filter(|r| {
            r.node.property_bool("is_test").unwrap_or(false) || is_test_path(&r.node.file_path)
        })
        .map(
            |r| json!({"qn": r.node.qualified_name, "name": r.node.name, "file": r.node.file_path}),
        )
        .collect();

    let changed_page: Vec<Value> = changed_rows
        .into_iter()
        .skip(req.changed_offset)
        .take(req.changed_limit.max(1))
        .collect();

    Ok(json!({
        "project": req.project,
        "generation": project.generation,
        "scope": req.scope,
        "base_branch": base,
        "git_available": true,
        "changed_files": {
            "total": changed_total,
            "returned": changed_page.len(),
            "offset": req.changed_offset,
            "has_more": req.changed_offset + changed_page.len() < changed_total,
            "next_offset": if req.changed_offset + changed_page.len() < changed_total {
                Some(req.changed_offset + changed_page.len())
            } else { None },
            "rows": changed_page,
        },
        "seeds": {
            "total": seeds.len(),
            "returned": seed_rows.len(),
            "offset": req.seed_offset,
            "has_more": req.seed_offset + seed_rows.len() < seeds.len(),
            "next_offset": if req.seed_offset + seed_rows.len() < seeds.len() {
                Some(req.seed_offset + seed_rows.len())
            } else { None },
            "rows": seed_rows,
        },
        "impact": {
            "total": impact_rows.len(),
            "returned": impact_json.len(),
            "offset": req.impact_offset,
            "has_more": req.impact_offset + impact_json.len() < impact_rows.len(),
            "next_offset": if req.impact_offset + impact_json.len() < impact_rows.len() {
                Some(req.impact_offset + impact_json.len())
            } else { None },
            "rows": impact_json,
        },
        "modules": {
            "total": modules_count(&changed, &impact_rows),
            "returned": module_rows.len(),
            "offset": req.module_offset,
            "has_more": req.module_offset + module_rows.len() < modules_count(&changed, &impact_rows),
            "rows": module_rows,
        },
        "tests": tests,
        "risk_counts": risk_counts,
        "direction": req.direction,
        "depth": req.depth,
        "note": "Impact is derived from indexed relationships; unresolved references and unindexed files can hide callers.",
    }))
}

#[derive(Debug, Clone)]
struct ImpactRow {
    node: Node,
    hop: usize,
    via_qn: String,
    via_edge: String,
    seed: String,
}

fn modules_count(changed: &[ChangedFile], impact: &[ImpactRow]) -> usize {
    let mut set = HashSet::new();
    for file in changed {
        set.insert(top_dir(&file.path));
    }
    for row in impact {
        set.insert(top_dir(&row.node.file_path));
    }
    set.len()
}

fn top_dir(path: &str) -> String {
    path.split('/').next().unwrap_or(".").to_string()
}

fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("__tests__")
        || p.ends_with("_test.go")
        || p.ends_with("_test.py")
        || p.ends_with("_test.rs")
        || p.ends_with(".test.ts")
        || p.ends_with(".spec.ts")
        || p.ends_with(".test.js")
}

fn degree_maps(
    store: &Store,
    project: &str,
) -> gry_core::GryResult<(HashMap<i64, i64>, HashMap<i64, i64>)> {
    let edges = store.all_edges(project)?;
    let mut in_map = HashMap::new();
    let mut out_map = HashMap::new();
    for edge in &edges {
        if matches!(
            edge.edge_type.as_str(),
            "CALLS" | "USAGE" | "CALL_REFERENCE" | "INHERITS" | "IMPLEMENTS"
        ) {
            *in_map.entry(edge.target_id).or_insert(0) += 1;
            *out_map.entry(edge.source_id).or_insert(0) += 1;
        }
    }
    Ok((in_map, out_map))
}

pub fn changed_paths(
    root: &Path,
    base: Option<&str>,
    since: Option<&str>,
) -> gry_core::GryResult<Vec<String>> {
    let base = base
        .map(str::to_string)
        .or_else(|| detect_base_branch(root));
    Ok(changed_files(root, base.as_deref(), since)?
        .into_iter()
        .map(|f| f.path)
        .collect())
}

pub fn detect_base_branch(root: &Path) -> Option<String> {
    if let Some(head) = run_git(root, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        if let Some(name) = head.trim().rsplit('/').next() {
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    for candidate in ["main", "master"] {
        if run_git(root, &["rev-parse", "--verify", candidate]).is_some() {
            return Some(candidate.to_string());
        }
    }
    run_git(root, &["rev-parse", "--abbrev-ref", "@{upstream}"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn changed_files(
    root: &Path,
    base: Option<&str>,
    since: Option<&str>,
) -> gry_core::GryResult<Vec<ChangedFile>> {
    if run_git(root, &["rev-parse", "--git-dir"]).is_none() {
        return Err(gry_core::GryError::invalid(
            "GIT_UNAVAILABLE: the project root is not a Git repository",
        ));
    }
    let mut files: BTreeMap<String, ChangedFile> = BTreeMap::new();
    if let Some(since) = since {
        let out = run_git(
            root,
            &["diff", "--name-status", "-M", &format!("--since={since}")],
        )
        .unwrap_or_default();
        parse_name_status(&out, &mut files);
    } else if let Some(base) = base {
        // Prefer a merge-base comparison; an invalid ref is an error rather
        // than silently "no changes".
        let range = run_git(root, &["merge-base", base, "HEAD"])
            .map(|mb| format!("{}...HEAD", mb.trim()))
            .unwrap_or_else(|| format!("{base}...HEAD"));
        let out = run_git(root, &["diff", "--name-status", "-M", &range]).ok_or_else(|| {
            gry_core::GryError::invalid(format!(
                "GIT_UNAVAILABLE: cannot diff against base ref {base}"
            ))
        })?;
        parse_name_status(&out, &mut files);
        let work = run_git(root, &["diff", "--name-status", "-M"]).unwrap_or_default();
        parse_name_status(&work, &mut files);
    } else {
        let work = run_git(root, &["diff", "--name-status", "-M"]).unwrap_or_default();
        parse_name_status(&work, &mut files);
    }
    // Untracked files.
    let untracked =
        run_git(root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    for line in untracked.lines().filter(|l| !l.trim().is_empty()) {
        files.entry(line.to_string()).or_insert(ChangedFile {
            path: line.to_string(),
            old_path: None,
            status: "A".to_string(),
            hunks: Vec::new(),
        });
    }
    // Hunk mapping for modified files.
    let mut hunks: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
    if let Some(base) = base {
        if let Some(range) = run_git(root, &["merge-base", base, "HEAD"]) {
            let out = run_git(
                root,
                &[
                    "diff",
                    "-U0",
                    "--no-color",
                    &format!("{}...HEAD", range.trim()),
                ],
            )
            .unwrap_or_default();
            parse_hunks(&out, &mut hunks);
        }
    }
    let work = run_git(root, &["diff", "-U0", "--no-color"]).unwrap_or_default();
    parse_hunks(&work, &mut hunks);
    let mut out: Vec<ChangedFile> = files.into_values().collect();
    for file in &mut out {
        if let Some(h) = hunks.remove(&file.path) {
            file.hunks = h;
        }
        if file.status == "A" || file.status == "D" {
            file.hunks.clear();
        }
    }
    // Runtime state is not a repository change. The index already excludes
    // these directories; the Git view must agree with it.
    out.retain(|file| !is_runtime_state_path(&file.path));
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.truncate(2000);
    Ok(out)
}

fn is_runtime_state_path(path: &str) -> bool {
    let top = path.split('/').next().unwrap_or(path);
    matches!(
        top,
        ".git"
            | ".ohgo"
            | ".izen"
            | ".crush"
            | ".aider"
            | ".codex"
            | ".claude"
            | ".cursor"
            | "node_modules"
            | "target"
            | ".venv"
            | "__pycache__"
    )
}

fn parse_name_status(out: &str, files: &mut BTreeMap<String, ChangedFile>) {
    for line in out.lines() {
        let mut parts = line.split('\t');
        let Some(status) = parts.next() else { continue };
        let status_char = status.chars().next().unwrap_or('M');
        if status_char == 'R' || status_char == 'C' {
            let Some(old) = parts.next() else { continue };
            let Some(new) = parts.next() else { continue };
            files.insert(
                new.to_string(),
                ChangedFile {
                    path: new.to_string(),
                    old_path: Some(old.to_string()),
                    status: status_char.to_string(),
                    hunks: Vec::new(),
                },
            );
        } else {
            let Some(path) = parts.next() else { continue };
            files.insert(
                path.to_string(),
                ChangedFile {
                    path: path.to_string(),
                    old_path: None,
                    status: status_char.to_string(),
                    hunks: Vec::new(),
                },
            );
        }
    }
}

fn parse_hunks(out: &str, hunks: &mut HashMap<String, Vec<(i64, i64)>>) {
    let mut current: Option<String> = None;
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current = Some(path.to_string());
            continue;
        }
        if line.starts_with("+++ ") {
            current = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            let Some(new_part) = rest.split(' ').find(|p| p.starts_with('+')) else {
                continue;
            };
            let new_part = new_part.trim_start_matches('+');
            let (start, count) = match new_part.split_once(',') {
                Some((s, c)) => (s.parse::<i64>().unwrap_or(0), c.parse::<i64>().unwrap_or(1)),
                None => (new_part.parse::<i64>().unwrap_or(0), 1),
            };
            if let Some(path) = &current {
                hunks
                    .entry(path.clone())
                    .or_default()
                    .push((start, (start + count - 1).max(start)));
            }
        }
    }
}

fn run_git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_state_paths_are_not_repository_changes() {
        assert!(is_runtime_state_path(".ohgo/progress.json"));
        assert!(is_runtime_state_path(".izen/runtime/snapshot.json"));
        assert!(is_runtime_state_path("node_modules/pkg/index.js"));
        assert!(!is_runtime_state_path("internal/server/routes.go"));
        assert!(!is_runtime_state_path("cmd/api/main.go"));
    }
}
