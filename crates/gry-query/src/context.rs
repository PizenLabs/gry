//! Bounded task context: resolve targets, collect task-relevant evidence, and
//! return a deterministic, size-bounded payload.
//!
//! This module composes existing query primitives (`resolve_node`, `trace`,
//! coverage, change detection) instead of introducing a second graph engine.
//! Every relationship keeps its provenance. A relationship is `verified` when
//! the index resolved it with non-ambiguous evidence, `declared` when it comes
//! from a manifest or configuration file, `inferred` when resolution was
//! ambiguous or heuristic, and `observed` when runtime traces recorded it.
//! None of these mean "runtime true"; they describe the evidence gry has.

use crate::impact::{changed_paths, detect_base_branch};
use gry_core::types::{labels, Node};
use gry_store::traverse::{Direction, TraceParams};
use gry_store::Store;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const CONTEXT_PROFILES: &[&str] = &["context", "scout", "auditor"];

/// Bumped when the meaning of context evidence changes in a way that makes a
/// previously cached payload unsafe to reuse. Hook caches carry this value.
pub const CONTEXT_CONTRACT_VERSION: u32 = 2;

/// Freshness is bounded. At most this many referenced paths are hash-checked
/// and this many of their parent directories are scanned for added files.
const FRESHNESS_MAX_PATHS: usize = 64;
const FRESHNESS_MAX_SCAN_DIRS: usize = 16;
const FRESHNESS_MAX_SCAN_ENTRIES: usize = 2000;
const FRESHNESS_MAX_HASH_BYTES: u64 = 8 * 1024 * 1024;
pub const CONTEXT_INCLUDES: &[&str] = &[
    "callers", "callees", "imports", "services", "tests", "changes", "coverage",
];

#[derive(Debug, Clone)]
pub struct ContextRequest {
    pub project: String,
    pub task: Option<String>,
    pub targets: Vec<String>,
    pub profile: String,
    pub depth: usize,
    pub max_nodes: usize,
    pub max_tokens: usize,
    pub include: Vec<String>,
    pub expected_generation: Option<String>,
    pub changed_paths: Vec<String>,
    pub timeout_ms: u64,
    pub task_id: Option<String>,
    pub stage: Option<String>,
}

impl Default for ContextRequest {
    fn default() -> Self {
        Self {
            project: String::new(),
            task: None,
            targets: Vec::new(),
            profile: "context".to_string(),
            depth: 2,
            max_nodes: 40,
            max_tokens: 3200,
            include: Vec::new(),
            expected_generation: None,
            changed_paths: Vec::new(),
            timeout_ms: 3000,
            task_id: None,
            stage: None,
        }
    }
}

pub fn get_task_context(store: &Store, req: &ContextRequest) -> gry_core::GryResult<Value> {
    let profile = req.profile.to_ascii_lowercase();
    if !CONTEXT_PROFILES.contains(&profile.as_str()) {
        return Err(gry_core::GryError::invalid(format!(
            "profile must be one of: {}",
            CONTEXT_PROFILES.join(", ")
        )));
    }
    let mut includes = normalize_includes(&req.include)?;
    if profile == "auditor" {
        for item in ["changes", "tests"] {
            if !includes.iter().any(|i| i == item) {
                includes.push(item.to_string());
            }
        }
    }
    if profile == "scout" && !includes.iter().any(|i| i == "coverage") {
        includes.push("coverage".to_string());
    }
    let changes_requested = includes.iter().any(|i| i == "changes");
    let record = store.require_project(&req.project)?;
    let root = Path::new(&record.root_path);
    let deadline = Deadline::new(req.timeout_ms);
    let mut warnings: Vec<Value> = Vec::new();
    let mut unknowns: Vec<Value> = Vec::new();

    let expected_mismatch = req
        .expected_generation
        .as_deref()
        .map(|expected| expected != record.generation)
        .unwrap_or(false);
    if expected_mismatch {
        warnings.push(json!({
            "code": "generation_mismatch",
            "message": "The caller expected a different generation; evidence may be stale.",
        }));
    }

    let mut target_inputs: Vec<String> = req
        .targets
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .take(32)
        .collect();
    let had_explicit_targets = !target_inputs.is_empty();
    let task_requested = !had_explicit_targets
        && req
            .task
            .as_deref()
            .map(|task| !task_words(task).is_empty())
            .unwrap_or(false);
    let mut task_resolved = false;
    if target_inputs.is_empty() {
        if let Some(task) = req.task.as_deref().filter(|t| !t.trim().is_empty()) {
            target_inputs = task_targets(store, &req.project, task, 5)?;
            task_resolved = !target_inputs.is_empty();
        }
    }
    let mut seen_inputs = HashSet::new();
    target_inputs.retain(|target| seen_inputs.insert(target.clone()));
    if target_inputs.is_empty() && req.changed_paths.is_empty() && !changes_requested {
        let (status, code, message) = if task_requested {
            (
                "not_found",
                "task_unresolved",
                "The task produced no indexed targets; this is a resolution failure, not an empty result.",
            )
        } else {
            (
                "empty",
                "no_targets",
                "Provide targets, a task, or changed_paths.",
            )
        };
        return Ok(minimal_payload(
            &req.project,
            &record.generation,
            &profile,
            status,
            req,
            vec![json!({"code": code, "message": message})],
        ));
    }

    let mut resolved_targets: Vec<ResolvedTarget> = Vec::new();
    for input in &target_inputs {
        if deadline.expired() {
            break;
        }
        resolved_targets.push(resolve_target(store, &req.project, input, task_resolved)?);
    }
    // Explicit changed paths and Git discovery seed an audit even when the
    // caller did not pass target symbols. This is the after_edit/before_finish
    // contract.
    let mut change_report = Value::Null;
    if changes_requested || profile == "auditor" || !req.changed_paths.is_empty() {
        let (report, paths) = collect_changes(store, req, root, &mut warnings);
        change_report = report;
        if !paths.is_empty() {
            for path in paths.iter().take(20) {
                let already = resolved_targets.iter().any(|t| {
                    t.file.as_deref() == Some(path.as_str())
                        || t.node
                            .as_ref()
                            .map(|n| n.file_path == *path)
                            .unwrap_or(false)
                });
                if already {
                    continue;
                }
                let target = resolve_file_target(store, &req.project, path)?;
                if target.is_resolved() {
                    resolved_targets.push(target);
                }
            }
        }
    }

    let resolved_count = resolved_targets.iter().filter(|t| t.is_resolved()).count();
    let unresolved_count = resolved_targets.len().saturating_sub(resolved_count);
    let unindexed_changes = change_report["rows"]
        .as_array()
        .map(|rows| rows.iter().filter(|r| r["indexed"] == false).count())
        .unwrap_or(0);
    if unindexed_changes > 0 {
        warnings.push(json!({
            "code": "changed_file_not_indexed",
            "count": unindexed_changes,
            "message": "Some changed files are not in the index; re-index before relying on their impact.",
        }));
    }

    let mut entries: BTreeMap<String, NodeEntry> = BTreeMap::new();
    for target in &resolved_targets {
        if let Some(node) = &target.node {
            insert_entry(
                &mut entries,
                node.clone(),
                "target",
                target.provenance.as_str(),
                0,
                Evidence::target(&node.qualified_name, target.provenance.as_str()),
            );
        }
        for member in target.members.iter().take(5) {
            insert_entry(
                &mut entries,
                member.clone(),
                "target_member",
                target.provenance.as_str(),
                0,
                Evidence::target(&member.qualified_name, target.provenance.as_str()),
            );
        }
    }

    let depth = req.depth.clamp(1, 5);
    let max_nodes = req.max_nodes.clamp(1, 120);
    let trace_limit = (max_nodes.saturating_mul(4)).clamp(8, 500);
    let edge_types = edge_types_for(&includes);
    let callers = includes.iter().any(|i| i == "callers");
    let callees = includes.iter().any(|i| i == "callees");
    let other_relationships = includes
        .iter()
        .any(|i| matches!(i.as_str(), "imports" | "services" | "tests"));
    let direction = if other_relationships {
        Direction::Both
    } else if callers && !callees {
        Direction::Inbound
    } else if callees && !callers {
        Direction::Outbound
    } else {
        Direction::Both
    };
    let mut truncated_traversal = false;
    let mut visited_total = 0usize;

    if !edge_types.is_empty() {
        'targets: for target in &resolved_targets {
            if deadline.expired() {
                warnings.push(
                json!({"code":"timeout","message":"Context assembly exceeded its time budget."}),
            );
                break;
            }
            let starts: Vec<&Node> = target
                .node
                .iter()
                .chain(target.members.iter().take(5))
                .collect();
            for start in starts {
                if entries.len() >= max_nodes.saturating_mul(2) {
                    truncated_traversal = true;
                    break 'targets;
                }
                let params = TraceParams {
                    project: req.project.clone(),
                    start_qn: start.qualified_name.clone(),
                    direction,
                    depth,
                    edge_types: edge_types.clone(),
                    include_tests: profile == "auditor"
                        || includes
                            .iter()
                            .any(|i| matches!(i.as_str(), "tests" | "callers" | "callees")),
                    limit: trace_limit,
                    parameter_name: None,
                };
                let trace = match store.trace(&params) {
                    Ok(Some(trace)) => trace,
                    Ok(None) => {
                        warnings.push(json!({
                        "code":"target_disappeared",
                        "message": format!("Target vanished during traversal: {}", start.qualified_name),
                    }));
                        continue;
                    }
                    Err(error) => {
                        warnings.push(json!({
                            "code":"lookup_failed",
                            "message": error.to_string(),
                            "target": start.qualified_name,
                        }));
                        continue;
                    }
                };
                truncated_traversal |= trace.truncated;
                visited_total += trace.visited;
                for hop in trace.hops {
                    if is_noise_label(&hop.node.label) {
                        continue;
                    }
                    let (role, _priority) = classify(&hop.via_edge, &hop.via_direction);
                    if role == "test"
                        && profile != "auditor"
                        && !includes.iter().any(|i| i == "tests")
                    {
                        continue;
                    }
                    let (source, target_qn) = if hop.via_direction == "outbound" {
                        (hop.via_qn.clone(), hop.node.qualified_name.clone())
                    } else {
                        (hop.node.qualified_name.clone(), hop.via_qn.clone())
                    };
                    let evidence = Evidence {
                        edge: hop.via_edge.clone(),
                        source,
                        target: target_qn,
                        direction: if hop.via_direction == "outbound" {
                            "outbound"
                        } else {
                            "inbound"
                        },
                        provenance: provenance_for(&hop.edge_properties, hop.observed),
                        confidence: hop
                            .edge_properties
                            .get("confidence")
                            .and_then(Value::as_str)
                            .and_then(|v| v.parse::<f64>().ok()),
                        resolved_by: hop
                            .edge_properties
                            .get("resolved_by")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        observed: hop.observed,
                        observed_count: hop.observed_count,
                    };
                    insert_entry(
                        &mut entries,
                        hop.node,
                        role,
                        evidence.provenance,
                        hop.hop,
                        evidence,
                    );
                }
            }
        }
    }

    if deadline.expired() && !warnings.iter().any(|warning| warning["code"] == "timeout") {
        warnings
            .push(json!({"code":"timeout","message":"Context assembly exceeded its time budget."}));
    }

    // Resolve unresolved targets into explicit unknowns; never present them as
    // an empty successful result.
    for target in &resolved_targets {
        if !target.is_resolved() {
            unknowns.push(json!({
                "kind": "unresolved_target",
                "input": target.input,
                "reason": target.reason.clone().unwrap_or_else(|| "not_found".to_string()),
            }));
        }
    }
    if task_requested && !task_resolved {
        unknowns.push(json!({
            "kind": "task_unresolved",
            "reason": "task produced no indexed targets",
        }));
    } else if !task_resolved
        && target_inputs.is_empty()
        && req.changed_paths.is_empty()
        && !changes_requested
    {
        unknowns.push(json!({"kind":"empty_task","reason":"task has no usable terms"}));
    }

    let mut node_rows: Vec<&NodeEntry> = entries.values().collect();
    node_rows.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.distance.cmp(&b.distance))
            .then(b.node.name.len().cmp(&a.node.name.len()))
            .then(a.node.qualified_name.cmp(&b.node.qualified_name))
    });
    let node_total = node_rows.len();
    let node_rows: Vec<&NodeEntry> = node_rows.into_iter().take(max_nodes).collect();
    if node_total > max_nodes {
        unknowns.push(json!({
            "kind": "nodes_truncated",
            "omitted": node_total - max_nodes,
        }));
    }
    let kept: HashSet<&str> = node_rows
        .iter()
        .map(|e| e.node.qualified_name.as_str())
        .collect();
    let kept: HashSet<String> = kept.into_iter().map(str::to_string).collect();

    let mut relationships: Vec<Value> = Vec::new();
    let mut rel_seen: HashSet<(String, String, String)> = HashSet::new();
    for entry in &node_rows {
        for evidence in entry.evidence.iter().take(3) {
            if evidence.edge == "target" {
                continue;
            }
            if !kept.contains(&evidence.source) && !kept.contains(&evidence.target) {
                continue;
            }
            let key = (
                evidence.source.clone(),
                evidence.edge.clone(),
                evidence.target.clone(),
            );
            if !rel_seen.insert(key) {
                continue;
            }
            relationships.push(evidence.json());
            if relationships.len() >= 120 {
                break;
            }
        }
    }
    relationships.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["source"].as_str().unwrap_or("").to_string(),
                v["type"].as_str().unwrap_or("").to_string(),
                v["target"].as_str().unwrap_or("").to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    let relationships_omitted = relationships.len().saturating_sub(120);
    relationships.truncate(120);
    if relationships_omitted > 0 {
        unknowns.push(json!({
            "kind": "relationships_truncated",
            "omitted": relationships_omitted,
        }));
    }

    let files = file_rows(&node_rows);
    let services = service_rows(&node_rows);
    let boundaries = boundary_rows(&node_rows);
    let tests = test_rows(&node_rows);
    let evidence_paths = referenced_paths(&resolved_targets, &node_rows, &req.changed_paths);
    let coverage = coverage_report(
        store,
        &req.project,
        &record,
        &node_rows,
        &resolved_targets,
        &req.changed_paths,
    )?;
    let mut freshness_check = verify_freshness(
        store,
        &req.project,
        root,
        &evidence_paths,
        &record.generation,
    );
    // Re-validate after the graph work: a file or the whole index can change
    // while a query is assembling evidence. The final payload must say so
    // rather than presenting evidence from two incompatible snapshots.
    freshness_check.post_changed = post_check_changes(root, &freshness_check.verified_meta);
    match store.project(&req.project) {
        Ok(Some(current)) if current.generation != record.generation => {
            freshness_check.generation_changed = true;
        }
        Ok(None) => freshness_check.generation_changed = true,
        _ => {}
    }
    let freshness = freshness_report(&freshness_check, &record.generation, expected_mismatch);

    if truncated_traversal {
        unknowns.push(json!({
            "kind": "traversal_truncated",
            "visited": visited_total,
            "message": "Traversal stopped at the depth, node or result bound.",
        }));
    }
    if !coverage["gaps"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true)
    {
        unknowns.push(json!({
            "kind": "coverage_gap",
            "message": "Some relevant files were partially parsed or could not be parsed; see coverage.gaps.",
        }));
    }
    if freshness["stale"].as_bool().unwrap_or(false) {
        unknowns.push(json!({
            "kind": "stale_index",
            "message": "Relevant files changed after indexing; re-index before trusting the result as current.",
        }));
    }

    let mut follow_up: Vec<Value> = Vec::new();
    for target in resolved_targets.iter().filter(|t| !t.is_resolved()).take(3) {
        follow_up.push(json!({
            "tool": "search_graph",
            "args": {"project": req.project, "query": target.input},
            "reason": "Target did not resolve; search before retrying.",
        }));
    }
    if truncated_traversal {
        if let Some(start) = resolved_targets.iter().find_map(|t| {
            t.node
                .as_ref()
                .or_else(|| t.members.first())
                .map(|n| n.qualified_name.clone())
        }) {
            follow_up.push(json!({
                "tool": "trace_path",
                "args": {"project": req.project, "function_name": start, "depth": (depth + 1).min(5)},
                "reason": "Raise depth or limit for the bounded traversal.",
            }));
        }
    }
    if !coverage["gaps"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true)
    {
        follow_up.push(json!({
            "tool": "check_index_coverage",
            "args": {"project": req.project},
            "reason": "Confirm whether the cited files were fully indexed.",
        }));
    }
    if profile == "auditor" {
        follow_up.push(json!({
            "tool": "detect_changes",
            "args": {"project": req.project, "scope": "tests"},
            "reason": "Test selection uses policy, coverage and Git history beyond this bounded audit.",
        }));
    }

    if deadline.expired() && !warnings.iter().any(|warning| warning["code"] == "timeout") {
        warnings
            .push(json!({"code":"timeout","message":"Context assembly exceeded its time budget."}));
    }
    let status = if deadline.expired() {
        "timeout"
    } else if freshness["stale"].as_bool().unwrap_or(false) || expected_mismatch {
        "stale"
    } else if resolved_count == 0 {
        if !warnings.is_empty() {
            "partial"
        } else if target_inputs.is_empty() && req.changed_paths.is_empty() && changes_requested {
            "empty"
        } else {
            "not_found"
        }
    } else if !warnings.is_empty() || unresolved_count > 0 || (task_requested && !task_resolved) {
        "partial"
    } else {
        "ok"
    };

    let audit = if profile == "auditor" {
        Some(audit_sections(
            &node_rows,
            &resolved_targets,
            &change_report,
            &coverage,
            &freshness,
            truncated_traversal,
            &warnings,
        ))
    } else {
        None
    };

    let mut out = Map::new();
    out.insert("project".into(), json!(req.project));
    out.insert("generation".into(), json!(record.generation));
    out.insert("profile".into(), json!(profile));
    if let Some(task_id) = &req.task_id {
        out.insert("task_id".into(), json!(task_id));
    }
    if let Some(stage) = &req.stage {
        out.insert("stage".into(), json!(stage));
    }
    out.insert("status".into(), json!(status));
    out.insert("freshness".into(), freshness.clone());
    out.insert(
        "targets".into(),
        json!(resolved_targets
            .iter()
            .map(|t| t.json())
            .collect::<Vec<_>>()),
    );
    out.insert(
        "nodes".into(),
        json!(node_rows.iter().map(|e| e.json()).collect::<Vec<_>>()),
    );
    out.insert("files".into(), json!(files));
    out.insert("relationships".into(), json!(relationships));
    out.insert("services".into(), json!(services));
    out.insert("boundaries".into(), json!(boundaries));
    out.insert("tests".into(), json!(tests));
    if !change_report.is_null() {
        out.insert("changes".into(), change_report);
    }
    out.insert("coverage".into(), coverage);
    let coverage_gaps = out["coverage"]["gaps"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    out.insert(
        "evidence".into(),
        evidence_contract(
            status,
            &freshness,
            truncated_traversal,
            coverage_gaps,
            false,
            node_total,
            node_rows.len(),
        ),
    );
    out.insert("unknowns".into(), json!(unknowns));
    out.insert("follow_up".into(), json!(follow_up));
    out.insert("warnings".into(), json!(warnings));
    out.insert(
        "note".into(),
        json!("Bounded evidence from the indexed graph. Absence of a relationship is not proof of safety; unresolved and inferred evidence is labeled."),
    );

    if profile == "scout" {
        out.insert(
            "map".into(),
            json!({
                "symbols": node_rows.iter().filter(|e| e.node.label != labels::FILE).take(12)
                    .map(|e| json!({"qn": e.node.qualified_name, "label": e.node.label, "role": e.role})).collect::<Vec<_>>(),
                "files": files.iter().take(8).cloned().collect::<Vec<_>>(),
                "services": services.iter().take(5).cloned().collect::<Vec<_>>(),
            }),
        );
    }
    if let Some((checks, verified, risks, missing)) = audit {
        out.insert("checks".into(), json!(checks));
        out.insert("verified_issues".into(), json!(verified));
        out.insert("possible_risks".into(), json!(risks));
        out.insert("missing_evidence".into(), json!(missing));
        out.insert(
            "safety".into(),
            json!("No affected relationship found is not proof that a change is safe."),
        );
    }

    let mut value = Value::Object(out);
    enforce_budget(&mut value, req.max_tokens);
    // Budget truncation removes evidence after the contract was computed.
    // Reflect that in the contract instead of leaving "complete" true.
    let truncated = value
        .get("budget")
        .and_then(|b| b.get("truncated"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if truncated {
        if let Some(evidence) = value.get_mut("evidence") {
            evidence["complete_for_scope"] = json!(false);
            evidence["safe_to_treat_as_complete"] = json!(false);
            if let Some(reasons) = evidence.get_mut("reasons").and_then(Value::as_array_mut) {
                reasons.push(json!("output_truncated"));
            }
        }
        // The extra fields are tiny, but re-run the hard bound when they push
        // the payload over. Preserve the truncation flag: the second pass
        // starts from the already-trimmed payload and would otherwise look
        // complete.
        let max_bytes = req.max_tokens.clamp(128, 1_000_000) * 4;
        if serde_json::to_string(&value)
            .map(|text| text.len() > max_bytes)
            .unwrap_or(true)
        {
            enforce_budget(&mut value, req.max_tokens);
            if let Value::Object(map) = &mut value {
                map.insert("truncated".into(), Value::Bool(true));
                if let Some(budget) = map.get_mut("budget").and_then(Value::as_object_mut) {
                    budget.insert("truncated".into(), Value::Bool(true));
                    if budget
                        .get("omitted")
                        .and_then(Value::as_object)
                        .map(|o| o.is_empty())
                        .unwrap_or(true)
                    {
                        budget.insert("omitted".into(), json!({"payload": 1}));
                    }
                }
            }
        }
    }
    Ok(value)
}

fn normalize_includes(requested: &[String]) -> gry_core::GryResult<Vec<String>> {
    if requested.is_empty() {
        return Ok(vec![
            "callers".to_string(),
            "callees".to_string(),
            "imports".to_string(),
            "services".to_string(),
            "coverage".to_string(),
        ]);
    }
    let mut out = Vec::new();
    for item in requested {
        let normalized = item.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if !CONTEXT_INCLUDES.contains(&normalized.as_str()) {
            return Err(gry_core::GryError::invalid(format!(
                "unknown include: {item}; valid values: {}",
                CONTEXT_INCLUDES.join(", ")
            )));
        }
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    if out.is_empty() {
        return Err(gry_core::GryError::invalid(
            "include must contain at least one supported value",
        ));
    }
    Ok(out)
}

fn edge_types_for(includes: &[String]) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    let mut add = |items: &[&str]| {
        for item in items {
            if !types.iter().any(|t| t == item) {
                types.push((*item).to_string());
            }
        }
    };
    if includes.iter().any(|i| i == "callers" || i == "callees") {
        add(&[
            "CALLS",
            "CALL_REFERENCE",
            "USAGE",
            "HTTP_CALLS",
            "GRPC_CALLS",
            "ASYNC_CALLS",
            "INHERITS",
            "IMPLEMENTS",
            "OVERRIDE",
            "HANDLES",
        ]);
    }
    if includes.iter().any(|i| i == "imports") {
        add(&["IMPORTS", "REFERENCES_FILE"]);
    }
    if includes.iter().any(|i| i == "services") {
        add(&[
            "DEPENDS_ON",
            "CONNECTS_TO",
            "INFRA_MAPS",
            "DEPLOYS",
            "ROUTES_TO",
            "USES_CONFIG",
            "USES_SECRET",
            "USES_NETWORK",
            "USES_VOLUME",
            "CROSS_HTTP_CALLS",
            "CROSS_GRPC_CALLS",
            "CROSS_ASYNC_CALLS",
            "CROSS_CHANNEL",
            "CONFIGURES",
            "SPAWNS",
            "HANDLES",
        ]);
    }
    if includes.iter().any(|i| i == "tests") {
        add(&["TESTS", "TESTS_FILE"]);
    }
    types
}

fn classify(edge: &str, direction: &str) -> (&'static str, u8) {
    let outbound = direction == "outbound";
    match edge {
        "CALLS" | "CALL_REFERENCE" | "HTTP_CALLS" | "GRPC_CALLS" | "ASYNC_CALLS" => {
            (if outbound { "callee" } else { "caller" }, 1)
        }
        "USAGE" => (if outbound { "usage" } else { "used_by" }, 2),
        "INHERITS" | "IMPLEMENTS" | "OVERRIDE" => (if outbound { "base" } else { "subtype" }, 2),
        "IMPORTS" => (if outbound { "dependency" } else { "importer" }, 2),
        "REFERENCES_FILE" => (
            if outbound {
                "references"
            } else {
                "referenced_by"
            },
            3,
        ),
        "TESTS" | "TESTS_FILE" => ("test", 4),
        "HANDLES" => (if outbound { "handled_route" } else { "handler" }, 1),
        "DEPENDS_ON" | "CONNECTS_TO" | "INFRA_MAPS" | "DEPLOYS" | "ROUTES_TO" | "USES_CONFIG"
        | "USES_SECRET" | "USES_NETWORK" | "USES_VOLUME" | "SPAWNS" | "CROSS_HTTP_CALLS"
        | "CROSS_GRPC_CALLS" | "CROSS_ASYNC_CALLS" | "CROSS_CHANNEL" => ("service", 2),
        "CONFIGURES" => ("config", 4),
        _ => ("related", 5),
    }
}

fn provenance_for(props: &Value, observed: bool) -> &'static str {
    if observed {
        return "observed";
    }
    if let Some(resolved_by) = props.get("resolved_by").and_then(Value::as_str) {
        if resolved_by == "ambiguous" || resolved_by == "inferred" {
            return "inferred";
        }
    }
    if props
        .get("ambiguous")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "inferred";
    }
    if matches!(
        props.get("ecosystem").and_then(Value::as_str),
        Some("infra")
            | Some("kubernetes")
            | Some("helm")
            | Some("compose")
            | Some("terraform")
            | Some("ansible")
            | Some("github_actions")
            | Some("make")
    ) || matches!(
        props.get("source").and_then(Value::as_str),
        Some("kubernetes") | Some("compose") | Some("dockerfile")
    ) || matches!(
        props.get("via").and_then(Value::as_str),
        Some("compose")
            | Some("compose_build")
            | Some("compose_env_file")
            | Some("compose_links")
            | Some("kubernetes")
            | Some("ingress")
            | Some("helm")
            | Some("terraform")
            | Some("ansible")
            | Some("selector")
            | Some("from")
            | Some("env")
            | Some("make")
            | Some("ci")
    ) {
        return "declared";
    }
    "verified"
}

fn is_noise_label(label: &str) -> bool {
    matches!(label, labels::PROJECT | labels::FOLDER | labels::PACKAGE)
}

fn is_test_node(node: &Node) -> bool {
    gry_store::traverse::is_test_node(node)
}

fn insert_entry(
    entries: &mut BTreeMap<String, NodeEntry>,
    node: Node,
    role: &'static str,
    provenance: &str,
    distance: usize,
    evidence: Evidence,
) {
    let provenance = static_provenance(provenance);
    let priority = if role == "target" {
        // Not every resolved target is equally useful. A route or callable is
        // the thing a task is about; a field/constant or a documentation
        // section is supporting evidence and must not crowd the call chain out
        // of a small output budget.
        target_label_priority(&node.label)
    } else {
        role_priority(role)
    };
    let key = node.qualified_name.clone();
    match entries.get_mut(&key) {
        Some(existing) => {
            if distance < existing.distance {
                existing.distance = distance;
            }
            if priority < existing.priority {
                existing.priority = priority;
                existing.role = role;
            }
            if existing.evidence.len() < 4
                && !existing.evidence.iter().any(|e| {
                    e.edge == evidence.edge
                        && e.source == evidence.source
                        && e.target == evidence.target
                })
            {
                existing.evidence.push(evidence);
            }
            existing.provenance = merge_provenance(existing.provenance, provenance);
        }
        None => {
            entries.insert(
                key,
                NodeEntry {
                    node,
                    role,
                    priority,
                    distance,
                    provenance,
                    evidence: vec![evidence],
                },
            );
        }
    }
}

fn static_provenance(provenance: &str) -> &'static str {
    match provenance {
        "verified" => "verified",
        "declared" => "declared",
        "observed" => "observed",
        _ => "inferred",
    }
}

fn merge_provenance(current: &'static str, next: &str) -> &'static str {
    let rank = |p: &str| match p {
        "verified" => 0,
        "declared" => 1,
        "observed" => 2,
        "inferred" => 3,
        _ => 4,
    };
    if rank(next) < rank(current) {
        match next {
            "verified" => "verified",
            "declared" => "declared",
            "observed" => "observed",
            _ => "inferred",
        }
    } else {
        current
    }
}

fn role_priority(role: &str) -> u8 {
    match role {
        "target" => 0,
        "target_member" => 1,
        "caller" | "callee" => 1,
        "dependency" | "importer" | "base" | "subtype" => 2,
        "usage" | "used_by" => 2,
        "service" => 2,
        "references" | "referenced_by" => 3,
        "config" => 4,
        "test" => 2,
        _ => 5,
    }
}

fn target_label_priority(label: &str) -> u8 {
    match label {
        labels::ROUTE | labels::FUNCTION | labels::METHOD | labels::CLASS | labels::INTERFACE => 0,
        labels::FILE | labels::MODULE | labels::PACKAGE | labels::FOLDER => 1,
        labels::FIELD | labels::CONSTANT | labels::VARIABLE => 2,
        "Section" | "Document" | "Doc" | "ADR" | "Heading" => 4,
        _ => 3,
    }
}

struct NodeEntry {
    node: Node,
    role: &'static str,
    priority: u8,
    distance: usize,
    provenance: &'static str,
    evidence: Vec<Evidence>,
}

impl NodeEntry {
    fn json(&self) -> Value {
        let mut out = json!({
            "qn": self.node.qualified_name,
            "name": self.node.name,
            "label": self.node.label,
            "file": self.node.file_path,
            "lines": [self.node.start_line, self.node.end_line],
            "role": self.role,
            "distance": self.distance,
            "provenance": self.provenance,
            "evidence": self.evidence.iter().take(1).map(Evidence::json).collect::<Vec<_>>(),
        });
        if is_test_node(&self.node) {
            out["is_test"] = json!(true);
        }
        if let Some(kind) = self.node.property_str("kind") {
            out["kind"] = json!(kind);
        }
        if let Some(namespace) = self.node.property_str("namespace") {
            out["namespace"] = json!(namespace);
        }
        out
    }
}

struct Evidence {
    edge: String,
    source: String,
    target: String,
    direction: &'static str,
    provenance: &'static str,
    confidence: Option<f64>,
    resolved_by: Option<String>,
    observed: bool,
    observed_count: i64,
}

impl Evidence {
    fn target(qn: &str, provenance: &str) -> Self {
        Self {
            edge: "target".to_string(),
            source: qn.to_string(),
            target: qn.to_string(),
            direction: "self",
            provenance: match provenance {
                "verified" => "verified",
                "inferred" => "inferred",
                _ => "verified",
            },
            confidence: None,
            resolved_by: None,
            observed: false,
            observed_count: 0,
        }
    }

    fn json(&self) -> Value {
        let mut out = json!({
            "source": self.source,
            "target": self.target,
            "type": self.edge,
            "direction": self.direction,
            "provenance": self.provenance,
            "observed": self.observed,
        });
        if let Some(confidence) = self.confidence {
            out["confidence"] = json!(confidence);
        }
        if let Some(resolved_by) = &self.resolved_by {
            out["resolved_by"] = json!(resolved_by);
        }
        if self.observed {
            out["observed_count"] = json!(self.observed_count);
        }
        out
    }
}

struct ResolvedTarget {
    input: String,
    method: String,
    provenance: String,
    node: Option<Node>,
    members: Vec<Node>,
    file: Option<String>,
    reason: Option<String>,
}

impl ResolvedTarget {
    fn is_resolved(&self) -> bool {
        self.node.is_some() || !self.members.is_empty() || self.file.is_some()
    }

    fn json(&self) -> Value {
        let mut out = json!({
            "input": self.input,
            "status": if self.is_resolved() { "resolved" } else { "unresolved" },
            "method": self.method,
            "provenance": self.provenance,
        });
        if let Some(node) = &self.node {
            out["qn"] = json!(node.qualified_name);
            out["label"] = json!(node.label);
            out["file"] = json!(node.file_path);
        }
        if let Some(file) = &self.file {
            out["file"] = json!(file);
        }
        if !self.members.is_empty() {
            out["members"] = json!(self.members.len());
        }
        if let Some(reason) = &self.reason {
            out["reason"] = json!(reason);
        }
        out
    }
}

fn resolve_target(
    store: &Store,
    project: &str,
    input: &str,
    from_task: bool,
) -> gry_core::GryResult<ResolvedTarget> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(ResolvedTarget {
            input: input.to_string(),
            method: "none".to_string(),
            provenance: "inferred".to_string(),
            node: None,
            members: Vec::new(),
            file: None,
            reason: Some("empty".to_string()),
        });
    }
    if let Some(node) = store.node_by_qn(project, trimmed)? {
        return Ok(ResolvedTarget {
            input: input.to_string(),
            method: if from_task { "task_search" } else { "qn" }.to_string(),
            provenance: if from_task { "inferred" } else { "verified" }.to_string(),
            node: Some(node),
            members: Vec::new(),
            file: None,
            reason: None,
        });
    }
    if let Some(node) = store.resolve_node(project, trimmed)? {
        return Ok(ResolvedTarget {
            input: input.to_string(),
            method: if from_task { "task_search" } else { "name" }.to_string(),
            provenance: if from_task { "inferred" } else { "verified" }.to_string(),
            node: Some(node),
            members: Vec::new(),
            file: None,
            reason: None,
        });
    }
    if looks_like_path(trimmed) {
        // A path is resolved as a file or reported as unindexed. Do not fall
        // through to BM25, which could turn a missing file into an unrelated
        // symbol match.
        return resolve_file_target(store, project, trimmed);
    }
    let hits = store.bm25(project, trimmed, None, None, None, None, 5, 0)?;
    let Some(hits) = hits else {
        return Ok(unresolved_target(input, "not_found"));
    };
    let Some(hit) = hits
        .hits
        .iter()
        .find(|hit| !is_container_label(&hit.node.label))
        .or_else(|| hits.hits.first())
        .cloned()
    else {
        return Ok(unresolved_target(input, "not_found"));
    };
    Ok(ResolvedTarget {
        input: input.to_string(),
        method: if from_task { "task_search" } else { "search" }.to_string(),
        provenance: "inferred".to_string(),
        node: Some(hit.node),
        members: Vec::new(),
        file: None,
        reason: None,
    })
}

fn resolve_file_target(
    store: &Store,
    project: &str,
    path: &str,
) -> gry_core::GryResult<ResolvedTarget> {
    let normalized = normalize_path(path);
    let nodes = store.nodes_by_file(project, &normalized)?;
    if nodes.is_empty() {
        return Ok(unresolved_target(path, "file_not_indexed"));
    }
    let file_node = nodes.iter().find(|n| n.label == labels::FILE).cloned();
    let mut members: Vec<Node> = nodes
        .iter()
        .filter(|n| !matches!(n.label.as_str(), "File" | "Folder" | "Project" | "Package"))
        .cloned()
        .collect();
    members.sort_by(|a, b| {
        a.start_line
            .cmp(&b.start_line)
            .then(a.qualified_name.cmp(&b.qualified_name))
    });
    members.truncate(12);
    Ok(ResolvedTarget {
        input: path.to_string(),
        method: "file".to_string(),
        provenance: "verified".to_string(),
        node: file_node,
        members,
        file: Some(normalized),
        reason: None,
    })
}

fn unresolved_target(input: &str, reason: &str) -> ResolvedTarget {
    ResolvedTarget {
        input: input.to_string(),
        method: "none".to_string(),
        provenance: "inferred".to_string(),
        node: None,
        members: Vec::new(),
        file: None,
        reason: Some(reason.to_string()),
    }
}

fn is_container_label(label: &str) -> bool {
    matches!(label, "Package" | "Project" | "Folder" | "File" | "Module")
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/') || value.contains('\\') || Path::new(value).extension().is_some()
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn task_words(task: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the",
        "a",
        "an",
        "to",
        "of",
        "in",
        "for",
        "and",
        "with",
        "how",
        "what",
        "where",
        "which",
        "this",
        "that",
        "is",
        "are",
        "do",
        "does",
        "can",
        "should",
        "add",
        "fix",
        "change",
        "update",
        "implement",
        "make",
        "use",
        "from",
        "on",
        "by",
        "it",
        "we",
        "i",
        "you",
        "be",
        "as",
        "at",
        "or",
        "not",
        "when",
        "then",
        "into",
        "about",
        "need",
        "want",
    ];
    let mut words: Vec<String> = Vec::new();
    for raw in
        task.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '/' && c != '.' && c != '-')
    {
        let word = raw.trim().to_ascii_lowercase();
        if word.len() < 2 || STOPWORDS.contains(&word.as_str()) {
            continue;
        }
        if !words.contains(&word) {
            words.push(word);
        }
        if words.len() >= 8 {
            break;
        }
    }
    words
}

fn task_targets(
    store: &Store,
    project: &str,
    task: &str,
    limit: usize,
) -> gry_core::GryResult<Vec<String>> {
    let words = task_words(task);
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let mut targets = Vec::new();
    let routes = task_http_routes(task);
    let requested_paths: HashSet<String> = routes.iter().map(|(_, path)| path.clone()).collect();
    for (method, path) in &routes {
        let qn = format!("__route__{method}__{path}");
        if store.resolve_node(project, &qn)?.is_some() && !targets.contains(&qn) {
            targets.push(qn);
        }
    }
    for word in &words {
        if looks_like_path(word) && !requested_paths.contains(word) {
            targets.push(word.clone());
        }
    }
    let query = words.join(" ");
    let mut queries = vec![query];
    queries.extend(words.iter().cloned());
    let mut docs: Vec<String> = Vec::new();
    for query in queries {
        if targets.len() >= limit {
            break;
        }
        let Some(hits) = store.bm25(project, &query, None, None, None, None, limit, 0)? else {
            continue;
        };
        for hit in &hits.hits {
            let node = &hit.node;
            // A task that names an HTTP method and path is asking about that
            // route, not every other route that shares the verb.
            if node.label == labels::ROUTE
                && !requested_paths.is_empty()
                && !requested_paths.contains(&node.name)
            {
                continue;
            }
            if is_container_label(&node.label) || is_doc_label(&node.label) {
                if !docs.contains(&node.qualified_name) {
                    docs.push(node.qualified_name.clone());
                }
                continue;
            }
            if !targets.contains(&node.qualified_name) {
                targets.push(node.qualified_name.clone());
            }
            if targets.len() >= limit {
                break;
            }
        }
    }
    // Documentation is useful tangential evidence, never a substitute for a
    // code target that matched the same task.
    for doc in docs {
        if targets.len() >= limit {
            break;
        }
        if !targets.contains(&doc) {
            targets.push(doc);
        }
    }
    Ok(targets)
}

fn task_http_routes(task: &str) -> Vec<(String, String)> {
    const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    let tokens: Vec<&str> = task.split_whitespace().collect();
    let mut out = Vec::new();
    for pair in tokens.windows(2) {
        let method = pair[0]
            .trim_matches(|c: char| !c.is_ascii_alphanumeric())
            .to_ascii_uppercase();
        if !METHODS.contains(&method.as_str()) {
            continue;
        }
        let path = pair[1].trim_matches(|c: char| {
            !(c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | ':' | '{' | '}'))
        });
        if !path.starts_with('/') {
            continue;
        }
        let entry = (method, path.to_string());
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

fn is_doc_label(label: &str) -> bool {
    matches!(
        label,
        "Section" | "Document" | "Doc" | "ADR" | "Heading" | "Paragraph"
    )
}

fn file_rows(nodes: &[&NodeEntry]) -> Vec<Value> {
    let mut map: BTreeMap<String, (u8, BTreeSet<String>, usize)> = BTreeMap::new();
    for entry in nodes {
        if entry.node.file_path.is_empty() {
            continue;
        }
        let row =
            map.entry(entry.node.file_path.clone())
                .or_insert((entry.priority, BTreeSet::new(), 0));
        row.0 = row.0.min(entry.priority);
        row.1.insert(entry.role.to_string());
        row.2 += 1;
    }
    let mut rows: Vec<Value> = map
        .into_iter()
        .map(|(path, (priority, roles, count))| {
            json!({
                "path": path,
                "priority": priority,
                "nodes": count,
                "roles": roles.into_iter().collect::<Vec<_>>(),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a["priority"]
            .as_u64()
            .cmp(&b["priority"].as_u64())
            .then(b["nodes"].as_u64().cmp(&a["nodes"].as_u64()))
            .then(a["path"].as_str().cmp(&b["path"].as_str()))
    });
    rows.truncate(40);
    rows
}

fn service_rows(nodes: &[&NodeEntry]) -> Vec<Value> {
    let mut rows: Vec<Value> = nodes
        .iter()
        .filter(|e| is_service_entry(e))
        .take(20)
        .map(|e| {
            json!({
                "name": e.node.name,
                "qn": e.node.qualified_name,
                "label": e.node.label,
                "kind": e.node.property_str("kind"),
                "namespace": e.node.property_str("namespace"),
                "file": e.node.file_path,
                "root": top_dir(&e.node.file_path),
                "role": e.role,
                "provenance": e.provenance,
                "evidence": e.evidence.iter().take(1).map(Evidence::json).collect::<Vec<_>>(),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a["name"]
            .as_str()
            .cmp(&b["name"].as_str())
            .then(a["qn"].as_str().cmp(&b["qn"].as_str()))
    });
    rows
}

fn top_dir(path: &str) -> &str {
    path.split('/').next().unwrap_or(".")
}

fn is_service_entry(entry: &NodeEntry) -> bool {
    (labels::is_infrastructure(&entry.node.label) && entry.node.label != labels::ENV_VAR)
        || entry
            .node
            .property_str("kind")
            .map(|kind| {
                matches!(
                    kind,
                    "Service"
                        | "Deployment"
                        | "StatefulSet"
                        | "DaemonSet"
                        | "Job"
                        | "CronJob"
                        | "Ingress"
                        | "Network"
                        | "Volume"
                        | "ConfigMap"
                        | "Secret"
                )
            })
            .unwrap_or(false)
}

fn boundary_rows(nodes: &[&NodeEntry]) -> Vec<Value> {
    let mut map: BTreeMap<String, (usize, BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    for entry in nodes.iter().filter(|e| is_service_entry(e)) {
        let root = top_dir(&entry.node.file_path);
        let row = map
            .entry(root.to_string())
            .or_insert((0, BTreeSet::new(), BTreeSet::new()));
        row.0 += 1;
        row.1.insert(
            entry
                .node
                .property_str("kind")
                .unwrap_or(entry.node.label.as_str())
                .to_string(),
        );
        row.2.insert(entry.node.file_path.clone());
    }
    let mut rows: Vec<Value> = map
        .into_iter()
        .map(|(root, (services, kinds, files))| {
            json!({
                "root": root,
                "services": services,
                "kinds": kinds.into_iter().take(12).collect::<Vec<_>>(),
                "files": files.into_iter().take(10).collect::<Vec<_>>(),
            })
        })
        .collect();
    rows.sort_by(|a, b| a["root"].as_str().cmp(&b["root"].as_str()));
    rows.truncate(20);
    rows
}

fn test_rows(nodes: &[&NodeEntry]) -> Vec<Value> {
    let mut rows: Vec<Value> = nodes
        .iter()
        .filter(|e| e.role == "test" || is_test_node(&e.node))
        .take(20)
        .map(|e| {
            json!({
                "qn": e.node.qualified_name,
                "name": e.node.name,
                "file": e.node.file_path,
                "role": e.role,
                "provenance": e.provenance,
            })
        })
        .collect();
    rows.sort_by(|a, b| a["qn"].as_str().cmp(&b["qn"].as_str()));
    rows
}

fn referenced_paths(
    targets: &[ResolvedTarget],
    nodes: &[&NodeEntry],
    explicit_changed: &[String],
) -> BTreeSet<String> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for target in targets {
        if let Some(file) = &target.file {
            paths.insert(file.clone());
        }
        if let Some(node) = &target.node {
            if !node.file_path.is_empty() {
                paths.insert(node.file_path.clone());
            }
        }
        for member in &target.members {
            if !member.file_path.is_empty() {
                paths.insert(member.file_path.clone());
            }
        }
    }
    for entry in nodes {
        if !entry.node.file_path.is_empty() {
            paths.insert(entry.node.file_path.clone());
        }
    }
    for path in explicit_changed {
        let path = normalize_path(path);
        if !path.is_empty() {
            paths.insert(path);
        }
    }
    paths
}

fn coverage_report(
    store: &Store,
    project: &str,
    record: &gry_store::ProjectRecord,
    nodes: &[&NodeEntry],
    targets: &[ResolvedTarget],
    explicit_changed: &[String],
) -> gry_core::GryResult<Value> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for entry in nodes {
        if !entry.node.file_path.is_empty() {
            paths.insert(entry.node.file_path.clone());
        }
    }
    for target in targets {
        if let Some(file) = &target.file {
            paths.insert(file.clone());
        }
        if let Some(node) = &target.node {
            if !node.file_path.is_empty() {
                paths.insert(node.file_path.clone());
            }
        }
    }
    for path in explicit_changed {
        let path = normalize_path(path);
        if !path.is_empty() {
            paths.insert(path);
        }
    }
    let total_paths = paths.len();
    let paths: Vec<String> = paths.into_iter().take(64).collect();
    let coverage = store.coverage_for_paths(project, &paths)?;
    let map: HashMap<&str, &str> = coverage
        .iter()
        .map(|c| (c.rel_path.as_str(), c.kind.as_str()))
        .collect();
    let mut gaps = Vec::new();
    for path in &paths {
        if let Some(kind) = map.get(path.as_str()) {
            if matches!(*kind, "parse_partial" | "parse_unusable") {
                gaps.push(json!({"path": path, "kind": kind}));
            }
        }
    }
    gaps.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(json!({
        "generation": record.generation,
        "total_paths": total_paths,
        "checked": paths.len(),
        "gaps": gaps,
        "meta": store.coverage_meta(project)?,
        "note": "A clean result means no recorded gap, not proof of completeness.",
    }))
}

/// What the bounded freshness check actually verified. The caller uses this to
/// report a precise claim instead of turning a sample into a repository-wide
/// guarantee.
#[derive(Debug, Default, Clone)]
pub(crate) struct FreshnessCheck {
    pub referenced: usize,
    pub checked: usize,
    pub unverified: usize,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub added: Vec<String>,
    pub unindexed: Vec<String>,
    pub stat_only: Vec<String>,
    pub scan_dirs: usize,
    pub scan_entries: usize,
    pub scan_limited: bool,
    pub post_changed: Vec<String>,
    pub generation_changed: bool,
    /// Metadata captured when a path was verified clean, used for the
    /// post-assembly concurrent-change check. Never serialized.
    pub verified_meta: Vec<(String, i64, i64)>,
}

impl FreshnessCheck {
    pub(crate) fn stale(&self) -> bool {
        self.generation_changed
            || !self.modified.is_empty()
            || !self.deleted.is_empty()
            || !self.added.is_empty()
            || !self.unindexed.is_empty()
            || !self.post_changed.is_empty()
    }
}

/// Verify the files that contributed evidence, plus the direct directories
/// they live in. Only a bounded sample is checked; `unverified` and
/// `scan_limited` carry the remainder so callers never mistake this for a
/// repository-wide scan.
pub(crate) fn verify_freshness(
    store: &Store,
    project: &str,
    root: &Path,
    paths: &BTreeSet<String>,
    generation: &str,
) -> FreshnessCheck {
    let mut check = FreshnessCheck {
        referenced: paths.len(),
        ..Default::default()
    };
    let checked_paths: Vec<String> = paths
        .iter()
        .filter(|p| !p.is_empty())
        .take(FRESHNESS_MAX_PATHS)
        .cloned()
        .collect();
    check.checked = checked_paths.len();
    check.unverified = paths.len().saturating_sub(checked_paths.len());

    let hashes: HashMap<String, gry_core::types::FileHash> = store
        .file_hashes_for_paths(project, &checked_paths)
        .unwrap_or_default()
        .into_iter()
        .map(|h| (h.rel_path.clone(), h))
        .collect();

    // Paths verified by content, kept for the post-assembly stat check.
    let mut verified: Vec<(String, i64, i64)> = Vec::new();
    for path in &checked_paths {
        let Some(hash) = hashes.get(path) else {
            // The path is part of the evidence but the index has no hash for
            // it. If the file exists, the index record is incomplete; if it is
            // absent, there is no hash to prove deletion, so count it as
            // unverified evidence rather than inventing a deletion.
            if root.join(path).exists() {
                check.unindexed.push(path.clone());
            } else {
                check.unverified += 1;
            }
            continue;
        };
        let metadata = match std::fs::metadata(root.join(path)) {
            Ok(metadata) => metadata,
            Err(_) => {
                check.deleted.push(path.clone());
                continue;
            }
        };
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let size = metadata.len() as i64;
        if mtime == hash.mtime_ns && size == hash.size {
            verified.push((path.clone(), mtime, size));
            continue;
        }
        // Stat differs. Content decides whether the evidence actually changed:
        // a touch or checkout that preserves bytes must not be reported as
        // stale evidence.
        match std::fs::read(root.join(path)) {
            Ok(bytes) if bytes.len() as u64 <= FRESHNESS_MAX_HASH_BYTES => {
                if gry_index::discover::sha256(&bytes) == hash.sha256 {
                    verified.push((path.clone(), mtime, size));
                } else {
                    check.modified.push(path.clone());
                }
            }
            _ => {
                // Too large to hash within the query budget. The stat mismatch
                // stands, so the result is conservatively stale.
                check.modified.push(path.clone());
            }
        }
    }

    // New or renamed files in the same directories are relevant to the same
    // task but have no hash row, so no referenced-path check can see them.
    let mut all_dirs: BTreeSet<String> = BTreeSet::new();
    for path in &checked_paths {
        if let Some(parent) = Path::new(path).parent() {
            let parent = parent.to_string_lossy().replace('\\', "/");
            if !parent.is_empty() {
                all_dirs.insert(parent);
            }
        }
    }
    if all_dirs.len() > FRESHNESS_MAX_SCAN_DIRS {
        check.scan_limited = true;
    }
    let dirs: Vec<String> = all_dirs.into_iter().take(FRESHNESS_MAX_SCAN_DIRS).collect();
    check.scan_dirs = dirs.len();
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    'dirs: for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            if check.scan_entries >= FRESHNESS_MAX_SCAN_ENTRIES {
                check.scan_limited = true;
                break 'dirs;
            }
            check.scan_entries += 1;
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = format!("{dir}/{name}");
            if checked_paths.iter().any(|p| p == &rel) {
                continue;
            }
            if is_ignored_scan_path(&rel) || gry_core::language::detect(&rel).is_none() {
                continue;
            }
            candidates.insert(rel);
        }
    }
    let candidate_paths: Vec<String> = candidates.into_iter().collect();
    let indexed: HashSet<String> = store
        .file_hashes_for_paths(project, &candidate_paths)
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.rel_path)
        .collect();
    let indexed_all: HashSet<String> = store
        .file_hashes(project)
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.rel_path)
        .collect();
    for path in candidate_paths {
        if !indexed.contains(&path) && !indexed_all.contains(&path) {
            check.added.push(path);
        }
    }

    check.modified.sort();
    check.deleted.sort();
    check.added.sort();
    check.unindexed.sort();
    check.stat_only.sort();
    check.verified_meta = verified;

    // Generation consistency: the graph may have been republished while this
    // query was assembling evidence. Re-read the generation and report the
    // uncertainty instead of silently combining two snapshots.
    match store.project(project) {
        Ok(Some(record)) if record.generation != generation => {
            check.generation_changed = true;
        }
        Ok(Some(_)) => {}
        Ok(None) => check.generation_changed = true,
        Err(_) => {}
    }
    check
}

/// Cheap post-assembly validation: if a verified file changed while the answer
/// was being built, the caller must not present the combination as one
/// snapshot. Only metadata is re-read, so this stays bounded.
pub(crate) fn post_check_changes(root: &Path, verified: &[(String, i64, i64)]) -> Vec<String> {
    let mut changed = Vec::new();
    for (path, mtime, size) in verified {
        let Ok(metadata) = std::fs::metadata(root.join(path)) else {
            changed.push(path.clone());
            continue;
        };
        let now_mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        if now_mtime != *mtime || metadata.len() as i64 != *size {
            changed.push(path.clone());
        }
    }
    changed.sort();
    changed
}

fn is_ignored_scan_path(path: &str) -> bool {
    const IGNORED_DIRS: &[&str] = &[
        ".git",
        ".ohgo",
        ".izen",
        ".crush",
        ".aider",
        ".codex",
        ".claude",
        ".cursor",
        "target",
        "node_modules",
        "dist",
        "build",
        "vendor",
        "third_party",
    ];
    path.split('/')
        .any(|part| IGNORED_DIRS.contains(&part) || part == ".DS_Store" || part.starts_with(".gry"))
}

fn freshness_report(check: &FreshnessCheck, generation: &str, expected_mismatch: bool) -> Value {
    let stale_paths: Vec<String> = check
        .modified
        .iter()
        .chain(check.deleted.iter())
        .chain(check.unindexed.iter())
        .chain(check.post_changed.iter())
        .cloned()
        .collect();
    let stale = expected_mismatch || check.stale();
    let freshness = if check.generation_changed {
        "generation_changed"
    } else if stale {
        "stale"
    } else if check.unverified > 0 || check.scan_limited {
        "unverified"
    } else {
        "fresh"
    };
    json!({
        "generation": generation,
        "expected_generation_mismatch": expected_mismatch,
        "generation_changed": check.generation_changed,
        "stale": stale,
        "freshness": freshness,
        "stale_paths": stale_paths,
        "modified_paths": check.modified,
        "deleted_paths": check.deleted,
        "added_paths": check.added,
        "unindexed_paths": check.unindexed,
        "changed_during_query": check.post_changed,
        "scope": "referenced_paths_and_directories",
        "repository_scan": false,
        "recursive_scan": false,
        "total_paths": check.referenced,
        "checked_paths": check.checked,
        "unverified_paths": check.unverified,
        "scan": {
            "directories": check.scan_dirs,
            "entries": check.scan_entries,
            "limited": check.scan_limited,
            "max_directories": FRESHNESS_MAX_SCAN_DIRS,
            "max_entries": FRESHNESS_MAX_SCAN_ENTRIES,
        },
        "check_method": "sha256_on_stat_mismatch",
        "limitations": [
            "Freshness covers referenced paths and files directly beside them; it is not a repository-wide scan.",
            "A change after the post-assembly check is outside this result's snapshot.",
        ],
    })
}

fn evidence_contract(
    status: &str,
    freshness: &Value,
    truncated_traversal: bool,
    coverage_gaps: usize,
    budget_truncated: bool,
    node_total: usize,
    node_returned: usize,
) -> Value {
    let mut reasons: Vec<&str> = Vec::new();
    if freshness["generation_changed"].as_bool().unwrap_or(false) {
        reasons.push("generation_changed_during_query");
    }
    if freshness["stale"].as_bool().unwrap_or(false) {
        reasons.push("stale_index");
    }
    if !freshness["added_paths"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true)
    {
        reasons.push("added_files_not_indexed");
    }
    if freshness["unverified_paths"].as_u64().unwrap_or(0) > 0
        || freshness["scan"]["limited"].as_bool().unwrap_or(false)
    {
        reasons.push("freshness_scope_bounded");
    }
    if truncated_traversal {
        reasons.push("traversal_bounded");
    }
    if coverage_gaps > 0 {
        reasons.push("coverage_gap");
    }
    if budget_truncated {
        reasons.push("output_truncated");
    }
    let query_ok = !matches!(status, "timeout" | "not_found" | "empty");
    let complete_for_scope = query_ok
        && status != "partial"
        && !freshness["stale"].as_bool().unwrap_or(false)
        && freshness["unverified_paths"].as_u64().unwrap_or(0) == 0
        && !freshness["scan"]["limited"].as_bool().unwrap_or(false)
        && !truncated_traversal
        && coverage_gaps == 0
        && node_returned >= node_total;
    json!({
        "query_ok": query_ok,
        "scope": "referenced_paths",
        "complete_for_scope": complete_for_scope,
        "safe_to_treat_as_complete": complete_for_scope,
        "repository_scan": false,
        "repository_complete": false,
        "freshness": freshness["freshness"],
        "reasons": reasons,
        "limitation": "Repository-wide completeness is not claimed. Use an explicit re-index for a global guarantee.",
    })
}

fn collect_changes(
    store: &Store,
    req: &ContextRequest,
    root: &Path,
    warnings: &mut Vec<Value>,
) -> (Value, Vec<String>) {
    let mut explicit: Vec<String> = req
        .changed_paths
        .iter()
        .map(|p| normalize_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    explicit.sort();
    explicit.dedup();
    let (paths, source, git_error) = if !explicit.is_empty() {
        (explicit, "caller", None)
    } else {
        match changed_paths(root, detect_base_branch(root).as_deref(), None) {
            Ok(paths) => (paths, "git", None),
            Err(error) => {
                let message = error.to_string();
                warnings.push(json!({"code":"git_unavailable","message": message}));
                (Vec::new(), "git", Some(message))
            }
        }
    };
    let bounded: Vec<String> = paths.iter().take(40).cloned().collect();
    let hashes = store
        .file_hashes_for_paths(&req.project, &bounded)
        .unwrap_or_default();
    let mut rows = Vec::new();
    for path in &bounded {
        let nodes = store.nodes_by_file(&req.project, path).unwrap_or_default();
        let indexed = hashes.iter().any(|h| h.rel_path == *path);
        rows.push(json!({
            "path": path,
            "indexed": indexed,
            "exists": root.join(path).exists(),
            "symbols": nodes.iter().filter(|n| !matches!(n.label.as_str(), "File" | "Folder" | "Project" | "Package")).count(),
        }));
    }
    let total = paths.len();
    let report = json!({
        "available": git_error.is_none() || source == "caller",
        "source": source,
        "total": total,
        "returned": rows.len(),
        "has_more": rows.len() < total,
        "next_offset": if rows.len() < total { Some(rows.len()) } else { None },
        "rows": rows,
        "reason": git_error,
    });
    (report, paths)
}

fn audit_sections(
    nodes: &[&NodeEntry],
    targets: &[ResolvedTarget],
    changes: &Value,
    coverage: &Value,
    freshness: &Value,
    truncated_traversal: bool,
    warnings: &[Value],
) -> (Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>) {
    let timed_out = warnings.iter().any(|w| w["code"] == "timeout");
    let lookup_failed = warnings.iter().any(|w| w["code"] == "lookup_failed");
    let unresolved = targets.iter().filter(|t| !t.is_resolved()).count();
    let gaps = coverage["gaps"].as_array().map(|a| a.len()).unwrap_or(0);
    let stale = freshness["stale"].as_bool().unwrap_or(false);
    let traversal_status = if timed_out || truncated_traversal || lookup_failed {
        "partial"
    } else {
        "checked"
    };
    let traversal_reason = if timed_out {
        Some("Context assembly timed out; only completed lookups are included.")
    } else if truncated_traversal {
        Some("Traversal hit its depth, node or result bound.")
    } else if lookup_failed {
        Some("One or more optional lookups failed; see warnings.")
    } else {
        None
    };
    let mut checks = vec![
        json!({
            "name":"target_resolution",
            "status": if unresolved > 0 { "partial" } else { "checked" },
            "reason": if unresolved > 0 { Some(format!("{unresolved} target(s) unresolved")) } else { None },
        }),
        json!({
            "name":"callers_callees",
            "status": traversal_status,
            "reason": traversal_reason,
        }),
        json!({"name":"imports","status": traversal_status, "reason": traversal_reason}),
        json!({"name":"infrastructure_relationships","status": traversal_status, "reason": traversal_reason}),
        json!({"name":"tests_graph_edges","status": traversal_status, "reason": traversal_reason}),
        json!({
            "name":"coverage",
            "status": if gaps > 0 { "partial" } else { "checked" },
            "reason": if gaps > 0 { Some(format!("{gaps} relevant file(s) have a recorded coverage gap")) } else { None },
        }),
        json!({
            "name":"freshness",
            "status": if stale { "partial" } else { "checked" },
            "reason": if stale { Some("The index is stale or the expected generation differs.") } else { None },
        }),
    ];
    if changes["available"].as_bool() == Some(false) {
        checks.push(json!({
            "name":"git_changes",
            "status":"skipped",
            "reason": changes["reason"].clone(),
        }));
    } else {
        checks.push(json!({"name":"git_changes","status":"checked"}));
    }
    checks.push(json!({
        "name":"test_selection_policy",
        "status":"skipped",
        "reason":"Use detect_changes(scope=tests) for policy, coverage and Git-history selection.",
    }));

    let mut verified = Vec::new();
    let mut risks = Vec::new();
    let mut missing = Vec::new();

    for target in targets.iter().filter(|t| !t.is_resolved()) {
        verified.push(json!({
            "kind":"unresolved_target",
            "input": target.input,
            "reason": target.reason,
        }));
        missing.push(json!({"kind":"unresolved_target","input": target.input}));
    }
    if let Some(rows) = changes["rows"].as_array() {
        for row in rows {
            if row["indexed"].as_bool() == Some(false) {
                verified.push(json!({
                    "kind":"changed_file_not_indexed",
                    "path": row["path"],
                }));
                missing.push(json!({
                    "kind":"changed_file_not_indexed",
                    "path": row["path"],
                    "action":"Re-index before relying on impact evidence.",
                }));
            }
        }
    }
    if freshness["stale"].as_bool().unwrap_or(false) {
        verified.push(json!({
            "kind":"stale_index",
            "paths": freshness["stale_paths"],
        }));
        missing.push(json!({"kind":"stale_index","paths": freshness["stale_paths"]}));
    }
    if let Some(gaps) = coverage["gaps"].as_array() {
        for gap in gaps.iter().take(10) {
            verified
                .push(json!({"kind":"partial_parse","path": gap["path"], "coverage": gap["kind"]}));
            missing.push(json!({"kind":"partial_parse","path": gap["path"]}));
        }
    }
    for entry in nodes.iter() {
        for evidence in &entry.evidence {
            if evidence.provenance == "inferred" && evidence.edge != "target" {
                verified.push(json!({
                    "kind":"ambiguous_relationship",
                    "source": evidence.source,
                    "target": evidence.target,
                    "type": evidence.edge,
                }));
            }
        }
    }
    for entry in nodes.iter().take(30) {
        if matches!(entry.role, "caller" | "used_by" | "importer" | "subtype")
            && entry.distance <= 1
        {
            risks.push(json!({
                "kind":"inbound_dependency",
                "qn": entry.node.qualified_name,
                "file": entry.node.file_path,
                "role": entry.role,
            }));
        }
        if entry.role == "service" {
            risks.push(json!({
                "kind":"infrastructure_relationship",
                "qn": entry.node.qualified_name,
                "file": entry.node.file_path,
                "message":"Static manifest relationship; runtime topology is not verified.",
            }));
        }
    }
    if !nodes
        .iter()
        .any(|e| e.role == "test" || is_test_node(&e.node))
    {
        missing.push(json!({
            "kind":"no_test_evidence",
            "message":"No test node reached within the bounded traversal; this is missing evidence, not proof that tests are unnecessary.",
        }));
    }
    risks.truncate(20);
    missing.truncate(20);
    verified.truncate(20);
    (checks, verified, risks, missing)
}

fn minimal_payload(
    project: &str,
    generation: &str,
    profile: &str,
    status: &str,
    req: &ContextRequest,
    unknowns: Vec<Value>,
) -> Value {
    let mut out = json!({
        "project": project,
        "generation": generation,
        "profile": profile,
        "status": status,
        "targets": [],
        "nodes": [],
        "files": [],
        "relationships": [],
        "services": [],
        "tests": [],
        "unknowns": unknowns,
        "follow_up": [],
        "warnings": [],
        "evidence": {
            "query_ok": !matches!(status, "timeout" | "not_found" | "empty"),
            "scope": "referenced_paths",
            "complete_for_scope": false,
            "safe_to_treat_as_complete": false,
            "repository_scan": false,
            "repository_complete": false,
            "freshness": "not_checked",
            "reasons": [status],
            "limitation": "No evidence was assembled, so completeness is not claimed.",
        },
        "budget": {
            "max_tokens": req.max_tokens,
            "max_bytes": req.max_tokens * 4,
        },
        "note": "Bounded evidence from the indexed graph; absence is not proof of safety.",
    });
    enforce_budget(&mut out, req.max_tokens);
    out
}

/// Deterministic whole-array truncation. Lower-value evidence is dropped first;
/// `budget.omitted` records exactly what was removed.
fn enforce_budget(value: &mut Value, max_tokens: usize) {
    let max_tokens = max_tokens.clamp(128, 1_000_000);
    let max_bytes = max_tokens * 4;
    let mut omitted: BTreeMap<String, usize> = BTreeMap::new();

    // Reserve room for the budget envelope itself; otherwise a payload that
    // just fits before the envelope is added would fall back to the minimal
    // payload even though most evidence is still useful.
    let reserve = (max_bytes / 8).clamp(64, 1024);
    trim_arrays(value, max_bytes.saturating_sub(reserve), &mut omitted);

    let size = |v: &Value| {
        serde_json::to_string(v)
            .map(|s| s.len())
            .unwrap_or(usize::MAX)
    };
    let truncated = !omitted.is_empty();
    let budget = |bytes: usize, omitted: &BTreeMap<String, usize>| {
        json!({
            "max_tokens": max_tokens,
            "max_bytes": max_bytes,
            "estimated_tokens": bytes.div_ceil(4),
            "estimated_bytes": bytes,
            "truncated": !omitted.is_empty(),
            "omitted": omitted,
        })
    };
    let mut bytes = size(value);
    if let Value::Object(map) = value {
        map.insert("budget".into(), budget(bytes, &omitted));
        if truncated {
            map.insert("truncated".into(), Value::Bool(true));
        }
    }
    if size(value) > max_bytes {
        trim_arrays(value, max_bytes.saturating_sub(64), &mut omitted);
        bytes = size(value);
        if let Value::Object(map) = value {
            map.insert("budget".into(), budget(bytes, &omitted));
            if !omitted.is_empty() {
                map.insert("truncated".into(), Value::Bool(true));
            }
        }
    }
    if size(value) <= max_bytes {
        return;
    }

    // Tiny budgets cannot hold the full envelope. Return an actionable,
    // deterministic minimal payload rather than invalid JSON or an empty error.
    let project = value
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("ok")
        .to_string();
    let minimal = json!({
        "project": project,
        "status": status,
        "truncated": true,
        "budget": {
            "max_tokens": max_tokens,
            "max_bytes": max_bytes,
            "estimated_tokens": 0,
            "estimated_bytes": 0,
            "truncated": true,
            "omitted": omitted,
        },
        "note": "Budget too small for evidence; raise max_output_tokens.",
    });
    *value = minimal;
    let bytes = size(value);
    if let Value::Object(map) = value {
        if let Some(budget) = map.get_mut("budget").and_then(Value::as_object_mut) {
            budget.insert("estimated_bytes".into(), json!(bytes));
            budget.insert("estimated_tokens".into(), json!(bytes.div_ceil(4)));
        }
    }
    if size(value) > max_bytes {
        // Pathological budgets (for example a very long project name) still
        // must not exceed the declared ceiling.
        *value = json!({"truncated": true, "reason": "max_output_tokens too small"});
    }
}

fn trim_arrays(value: &mut Value, target: usize, omitted: &mut BTreeMap<String, usize>) {
    let size = |v: &Value| {
        serde_json::to_string(v)
            .map(|s| s.len())
            .unwrap_or(usize::MAX)
    };

    // Trim whole nodes from the lowest-priority end and remove every
    // relationship that touches them. This keeps the returned graph coherent
    // instead of leaving dangling evidence.
    while size(value) > target {
        let Some(nodes) = value.get("nodes").and_then(Value::as_array) else {
            break;
        };
        if nodes.len() <= 2 {
            // Keep the resolved target plus at least one neighbour when the
            // budget allows it; a single-node answer loses the evidence that
            // made the target relevant. If even that is too large, the
            // minimal-payload fallback below takes over.
            break;
        }
        let Some(removed) = nodes
            .last()
            .and_then(|n| n["qn"].as_str())
            .map(str::to_string)
        else {
            break;
        };
        if let Some(Value::Array(nodes)) = value.get_mut("nodes") {
            nodes.pop();
        }
        *omitted.entry("nodes".to_string()).or_insert(0) += 1;
        if let Some(Value::Array(rels)) = value.get_mut("relationships") {
            let before = rels.len();
            rels.retain(|r| {
                r["source"].as_str() != Some(removed.as_str())
                    && r["target"].as_str() != Some(removed.as_str())
            });
            if rels.len() != before {
                *omitted.entry("relationships".to_string()).or_insert(0) += before - rels.len();
            }
        }
    }

    let order: &[&[&str]] = &[
        &["map", "symbols"],
        &["map", "files"],
        &["map", "services"],
        &["relationships"],
        &["files"],
        &["boundaries"],
        &["services"],
        &["tests"],
        &["changes", "rows"],
        &["unknowns"],
        &["follow_up"],
        &["possible_risks"],
        &["verified_issues"],
        &["checks"],
        &["coverage", "gaps"],
    ];
    for path in order {
        while size(value) > target {
            let len = get_path(value, path)
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            if len == 0 {
                break;
            }
            let new_len = len.saturating_sub(if len > 50 { 10 } else { 1 });
            if let Some(Value::Array(items)) = get_path_mut(value, path) {
                items.truncate(new_len);
            }
            let key = path.join(".");
            *omitted.entry(key).or_insert(0) += len - new_len;
        }
        if size(value) <= target {
            break;
        }
    }

    // Keep relationships consistent with the retained node set.
    if let Some(Value::Array(nodes)) = value.get("nodes") {
        let kept: HashSet<String> = nodes
            .iter()
            .filter_map(|n| n["qn"].as_str().map(str::to_string))
            .collect();
        if let Some(Value::Array(rels)) = value.get_mut("relationships") {
            let before = rels.len();
            rels.retain(|r| {
                r["source"]
                    .as_str()
                    .map(|s| kept.contains(s))
                    .unwrap_or(false)
                    && r["target"]
                        .as_str()
                        .map(|s| kept.contains(s))
                        .unwrap_or(false)
            });
            if rels.len() != before {
                *omitted.entry("relationships".to_string()).or_insert(0) += before - rels.len();
            }
        }
    }
}

fn get_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn get_path_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut current = value;
    for key in path {
        current = current.get_mut(*key)?;
    }
    Some(current)
}

struct Deadline {
    start: Instant,
    timeout: Duration,
}

impl Deadline {
    fn new(timeout_ms: u64) -> Self {
        Self {
            start: Instant::now(),
            timeout: Duration::from_millis(timeout_ms.min(30_000)),
        }
    }

    fn expired(&self) -> bool {
        self.start.elapsed() >= self.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_core::types::FileHash;
    use gry_core::types::NewNode;
    use gry_store::{EdgeSpec, ProjectWrite};

    fn store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let nodes = vec![
            node("Function", "main", "p.src.main.main", "src/main.rs", 1, 10),
            node(
                "Function",
                "handler",
                "p.src.api.handler",
                "src/api.rs",
                1,
                20,
            ),
            node("Function", "save", "p.src.db.save", "src/db.rs", 1, 10),
            node(
                "Resource",
                "api",
                "p.deploy::compose/Service/-/api",
                "deploy/compose.yml",
                1,
                1,
            ),
            node("EnvVar", "SECRET_TOKEN", "__env__SECRET_TOKEN", "", 0, 0),
            node(
                "Function",
                "test_handler",
                "p.tests.test_handler",
                "tests/test_handler.rs",
                1,
                5,
            ),
        ];
        let edges = vec![
            EdgeSpec::new("p.src.main.main", "p.src.api.handler", "CALLS")
                .property("confidence", "0.80")
                .property("resolved_by", "import"),
            EdgeSpec::new("p.src.api.handler", "p.src.db.save", "CALLS")
                .property("confidence", "0.45")
                .property("resolved_by", "ambiguous")
                .property("ambiguous", true),
            EdgeSpec::new("p.tests.test_handler", "p.src.api.handler", "TESTS"),
            EdgeSpec::new(
                "p.deploy::compose/Service/-/api",
                "p.src.api.handler",
                "CONNECTS_TO",
            )
            .property("via", "env"),
        ];
        let root = tempfile::tempdir().unwrap().keep();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/api.rs"), "0123456789").unwrap();
        std::fs::write(root.join("src/main.rs"), "0123456789").unwrap();
        let mut files = Vec::new();
        for rel in ["src/api.rs", "src/main.rs"] {
            let metadata = std::fs::metadata(root.join(rel)).unwrap();
            let bytes = std::fs::read(root.join(rel)).unwrap();
            let mtime_ns = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            files.push(FileHash {
                rel_path: rel.into(),
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

    fn node(label: &str, name: &str, qn: &str, file: &str, start: i32, end: i32) -> NewNode {
        NewNode::new(label, name, qn, file, start, end)
    }

    fn req(target: &str) -> ContextRequest {
        ContextRequest {
            project: "p".into(),
            targets: vec![target.into()],
            max_tokens: 3200,
            ..Default::default()
        }
    }

    #[test]
    fn resolves_targets_and_keeps_provenance() {
        let store = store();
        let out = get_task_context(&store, &req("handler")).unwrap();
        assert_eq!(out["status"], "ok");
        assert_eq!(out["targets"][0]["status"], "resolved");
        let nodes = out["nodes"].as_array().unwrap();
        assert!(nodes
            .iter()
            .any(|n| n["name"] == "main" && n["role"] == "caller"));
        assert!(nodes
            .iter()
            .any(|n| n["name"] == "save" && n["role"] == "callee"));
        let rels = out["relationships"].as_array().unwrap();
        assert!(rels
            .iter()
            .any(|r| r["type"] == "CALLS" && r["provenance"] == "verified"));
        assert!(rels
            .iter()
            .any(|r| r["type"] == "CALLS" && r["provenance"] == "inferred"));
        assert!(rels
            .iter()
            .any(|r| r["type"] == "CONNECTS_TO" && r["provenance"] == "declared"));
    }

    #[test]
    fn requested_tests_are_not_filtered_as_callers() {
        let store = store();
        let mut request = req("handler");
        request.include = vec!["tests".into()];
        let out = get_task_context(&store, &request).unwrap();
        assert!(out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["role"] == "test"));
        assert!(out["tests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "test_handler"));
    }

    #[test]
    fn budget_is_a_hard_limit_and_reports_omissions() {
        let store = store();
        let mut request = req("handler");
        request.max_tokens = 128;
        let out = get_task_context(&store, &request).unwrap();
        let text = serde_json::to_string(&out).unwrap();
        assert!(text.len() <= 128 * 4, "len={} text={text}", text.len());
        assert_eq!(out["budget"]["truncated"], true);
        assert!(!out["budget"]["omitted"].as_object().unwrap().is_empty());
    }

    #[test]
    fn mixed_resolved_and_unresolved_targets_are_partial() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            targets: vec!["handler".into(), "zzz_missing".into()],
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "partial");
        assert!(out["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["kind"] == "unresolved_target"));
    }

    #[test]
    fn unresolved_targets_are_not_an_empty_success() {
        let store = store();
        let out = get_task_context(&store, &req("does_not_exist_anywhere")).unwrap();
        assert_eq!(out["status"], "not_found");
        assert_eq!(out["targets"][0]["status"], "unresolved");
        assert!(out["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["kind"] == "unresolved_target"));
    }

    #[test]
    fn caller_only_context_does_not_collect_callees() {
        let store = store();
        let mut request = req("handler");
        request.include = vec!["callers".into()];
        let out = get_task_context(&store, &request).unwrap();
        let names: Vec<&str> = out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(names.contains(&"main"));
        assert!(!names.contains(&"save"));
    }

    #[test]
    fn callee_only_context_does_not_collect_callers() {
        let store = store();
        let mut request = req("handler");
        request.include = vec!["callees".into()];
        let out = get_task_context(&store, &request).unwrap();
        let names: Vec<&str> = out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(names.contains(&"save"));
        assert!(!names.contains(&"main"));
    }

    #[test]
    fn coverage_only_context_does_not_traverse_the_graph() {
        let store = store();
        let mut request = req("handler");
        request.include = vec!["coverage".into()];
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["nodes"].as_array().unwrap().len(), 1);
        assert!(out["relationships"].as_array().unwrap().is_empty());
    }

    #[test]
    fn missing_file_target_is_not_resolved_by_search() {
        let store = store();
        let out = get_task_context(&store, &req("src/does_not_exist.rs")).unwrap();
        assert_eq!(out["status"], "not_found");
        assert_eq!(out["targets"][0]["reason"], "file_not_indexed");
    }

    #[test]
    fn empty_task_is_distinct_from_resolution_failure() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            task: Some("the and of".into()),
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "empty");
    }

    #[test]
    fn explicit_targets_ignore_a_supplementary_task() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            targets: vec!["handler".into()],
            task: Some("zzzz_unknown_term".into()),
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "ok");
        assert!(!out["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["kind"] == "task_unresolved"));
    }

    #[test]
    fn unresolved_task_is_not_an_empty_result() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            task: Some("zzzz_unknown_term".into()),
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "not_found");
        assert!(out["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["code"] == "task_unresolved"));
    }

    #[test]
    fn unresolved_task_is_reported_even_with_changed_paths() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            task: Some("zzzz_unknown_term".into()),
            changed_paths: vec!["src/api.rs".into()],
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "partial");
        assert!(out["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["kind"] == "task_unresolved"));
    }

    #[test]
    fn unindexed_changed_file_is_partial_and_explicit() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            targets: vec!["handler".into()],
            changed_paths: vec!["src/new.rs".into()],
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "partial");
        assert!(out["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "changed_file_not_indexed"));
        assert_eq!(out["changes"]["rows"][0]["indexed"], false);
    }

    #[test]
    fn generation_mismatch_is_stale() {
        let store = store();
        let mut request = req("handler");
        request.expected_generation = Some("gen-2".into());
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "stale");
        assert_eq!(out["freshness"]["stale"], true);
        assert!(out["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "generation_mismatch"));
    }

    #[test]
    fn unknown_profile_and_include_are_invalid() {
        let store = store();
        let mut request = req("handler");
        request.profile = "universal".into();
        assert!(get_task_context(&store, &request).is_err());
        let mut request = req("handler");
        request.include = vec!["everything".into()];
        assert!(get_task_context(&store, &request).is_err());
    }

    #[test]
    fn scout_and_auditor_have_distinct_contracts() {
        let store = store();
        let mut request = req("handler");
        request.profile = "scout".into();
        let scout = get_task_context(&store, &request).unwrap();
        assert!(scout["map"]["symbols"].is_array());

        let mut request = req("handler");
        request.profile = "auditor".into();
        let audit = get_task_context(&store, &request).unwrap();
        assert!(audit["checks"].is_array());
        assert!(audit["verified_issues"].is_array());
        assert!(audit["safety"].as_str().unwrap().contains("not proof"));
        assert!(!audit.to_string().contains("safe_to_merge"));
    }

    #[test]
    fn context_never_returns_secret_values_or_properties() {
        let store = store();
        let mut request = req("handler");
        request.include = vec!["callers".into(), "callees".into(), "services".into()];
        let out = get_task_context(&store, &request).unwrap();
        let text = out.to_string();
        assert!(!text.contains("SECRET_TOKEN"));
        assert!(!text.contains("properties"));
    }

    #[test]
    fn completeness_is_separate_from_query_success() {
        let store = store();
        let out = get_task_context(&store, &req("handler")).unwrap();
        // The graph references files that have no hash record in this fixture,
        // so the query succeeds but completeness is not claimed.
        assert_eq!(out["status"], "ok");
        assert_eq!(out["evidence"]["query_ok"], true);
        assert_eq!(out["evidence"]["complete_for_scope"], false);
        assert_eq!(out["evidence"]["repository_scan"], false);
        assert_eq!(out["evidence"]["repository_complete"], false);
        assert!(out["freshness"]["unverified_paths"].as_u64().unwrap() > 0);
    }

    #[test]
    fn new_file_next_to_evidence_is_reported_added() {
        let store = store();
        let record = store.require_project("p").unwrap();
        std::fs::write(
            std::path::Path::new(&record.root_path).join("src/added.rs"),
            "pub fn added() {}\n",
        )
        .unwrap();
        let out = get_task_context(&store, &req("handler")).unwrap();
        assert_eq!(out["status"], "stale");
        assert_eq!(out["freshness"]["stale"], true);
        assert!(out["freshness"]["added_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/added.rs"));
        assert_eq!(out["evidence"]["complete_for_scope"], false);
        assert!(out["evidence"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "added_files_not_indexed"));
    }

    #[test]
    fn renamed_file_is_not_verified_current_evidence() {
        let store = store();
        let record = store.require_project("p").unwrap();
        let root = std::path::Path::new(&record.root_path);
        std::fs::rename(root.join("src/api.rs"), root.join("src/api_renamed.rs")).unwrap();
        let out = get_task_context(&store, &req("handler")).unwrap();
        assert_eq!(out["status"], "stale");
        assert!(out["freshness"]["deleted_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/api.rs"));
        assert!(out["freshness"]["added_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/api_renamed.rs"));
    }

    #[test]
    fn post_assembly_check_detects_a_concurrent_edit() {
        let store = store();
        let record = store.require_project("p").unwrap();
        let root = std::path::Path::new(&record.root_path);
        let mut paths = BTreeSet::new();
        paths.insert("src/api.rs".to_string());
        let check = verify_freshness(&store, "p", root, &paths, &record.generation);
        assert!(check.modified.is_empty());
        std::fs::write(root.join("src/api.rs"), "0123456789-changed").unwrap();
        let changed = post_check_changes(root, &check.verified_meta);
        assert!(changed.iter().any(|p| p == "src/api.rs"));
    }

    #[test]
    fn a_generation_change_during_a_check_is_explicit() {
        let store = store();
        let record = store.require_project("p").unwrap();
        let root = std::path::Path::new(&record.root_path);
        let mut paths = BTreeSet::new();
        paths.insert("src/api.rs".to_string());
        let check = verify_freshness(&store, "p", root, &paths, "older-generation");
        assert!(check.generation_changed);
        assert!(check.stale());
    }

    #[test]
    fn changed_caller_file_marks_the_index_stale() {
        let store = store();
        let record = store.require_project("p").unwrap();
        std::fs::write(
            std::path::Path::new(&record.root_path).join("src/main.rs"),
            "pub fn main() { changed(); }\n",
        )
        .unwrap();
        let out = get_task_context(&store, &req("handler")).unwrap();
        assert_eq!(out["status"], "stale");
        assert!(out["freshness"]["stale_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/main.rs"));
    }

    #[test]
    fn deleted_file_marks_the_index_stale() {
        let store = store();
        let record = store.require_project("p").unwrap();
        std::fs::remove_file(std::path::Path::new(&record.root_path).join("src/api.rs")).unwrap();
        let out = get_task_context(&store, &req("handler")).unwrap();
        assert_eq!(out["status"], "stale");
        assert!(out["freshness"]["stale_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/api.rs"));
    }

    #[test]
    fn timeout_keeps_a_structured_partial_result() {
        let store = store();
        let mut request = req("handler");
        request.timeout_ms = 0;
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["status"], "timeout");
        assert!(out["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "timeout"));
    }

    #[test]
    fn relationship_endpoints_are_retained_entities() {
        let store = store();
        let mut request = req("handler");
        request.max_tokens = 1200;
        let out = get_task_context(&store, &request).unwrap();
        let kept: HashSet<String> = out["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["qn"].as_str().map(str::to_string))
            .collect();
        for rel in out["relationships"].as_array().unwrap() {
            assert!(kept.contains(rel["source"].as_str().unwrap()));
            assert!(kept.contains(rel["target"].as_str().unwrap()));
        }
    }

    #[test]
    fn task_resolution_uses_bm25_and_marks_search_provenance() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            task: Some("handler request".into()),
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert!(out["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["method"] == "task_search"));
    }

    #[test]
    fn budget_payload_is_always_valid_json() {
        for tokens in [128usize, 160, 256, 512, 3200] {
            let store = store();
            let mut request = req("handler");
            request.max_tokens = tokens;
            let out = get_task_context(&store, &request).unwrap();
            let text = serde_json::to_string(&out).unwrap();
            assert!(serde_json::from_str::<Value>(&text).is_ok());
            assert!(
                text.len() <= tokens * 4,
                "tokens={tokens} len={}",
                text.len()
            );
        }
    }

    #[test]
    fn unicode_task_keeps_valid_json_and_a_hard_budget() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            task: Some("解析 handler 请求 🚀 — escape \"quotes\"".into()),
            max_tokens: 256,
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        let text = serde_json::to_string(&out).unwrap();
        assert!(serde_json::from_str::<Value>(&text).is_ok());
        assert!(text.len() <= 256 * 4, "len={}", text.len());
        assert!(out["budget"]["estimated_tokens"].as_u64().unwrap() <= 256);
    }

    #[test]
    fn explicit_changed_paths_seed_context_without_a_profile() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            changed_paths: vec!["src/api.rs".into()],
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["changes"]["source"], "caller");
        assert!(out["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["method"] == "file"));
    }

    #[test]
    fn explicit_changed_paths_seed_audits_without_git() {
        let store = store();
        let request = ContextRequest {
            project: "p".into(),
            profile: "auditor".into(),
            changed_paths: vec!["src/api.rs".into()],
            ..Default::default()
        };
        let out = get_task_context(&store, &request).unwrap();
        assert_eq!(out["changes"]["source"], "caller");
        assert_eq!(out["changes"]["total"], 1);
        assert!(out["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["method"] == "file"));
    }
}
