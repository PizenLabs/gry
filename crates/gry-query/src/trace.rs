//! Path tracing across calls, data flow, services and infrastructure.

use gry_core::types::{labels, Node};
use gry_store::traverse::{Direction, TraceParams};
use gry_store::Store;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct TraceRequest {
    pub project: String,
    pub function_name: String,
    pub direction: String,
    pub depth: usize,
    pub limit: usize,
    pub mode: String,
    pub parameter_name: Option<String>,
    pub edge_types: Vec<String>,
    pub risk_labels: bool,
    pub include_tests: bool,
    pub include_evidence: bool,
    pub cursor: Option<String>,
}

impl Default for TraceRequest {
    fn default() -> Self {
        Self {
            project: String::new(),
            function_name: String::new(),
            direction: "both".to_string(),
            depth: 3,
            limit: 100,
            mode: "calls".to_string(),
            parameter_name: None,
            edge_types: Vec::new(),
            risk_labels: false,
            include_tests: false,
            include_evidence: false,
            cursor: None,
        }
    }
}

pub fn trace_path(store: &Store, req: &TraceRequest) -> gry_core::GryResult<Value> {
    let project_record = store.require_project(&req.project)?;
    let start = resolve_start(store, &req.project, &req.function_name, &req.mode)?;
    let Some(start) = start else {
        // A short name can name several real symbols. Returning candidates is
        // actionable in one call; guessing the first would produce a confident
        // trace of the wrong function.
        let candidates = store.nodes_by_name(&req.project, req.function_name.trim())?;
        if candidates.len() > 1 {
            let rows: Vec<Value> = candidates
                .iter()
                .take(8)
                .map(|n| {
                    json!({
                        "qn": n.qualified_name,
                        "name": n.name,
                        "label": n.label,
                        "file": n.file_path,
                        "lines": [n.start_line, n.end_line],
                    })
                })
                .collect();
            return Ok(json!({
                "project": req.project,
                "found": false,
                "ambiguous": true,
                "message": format!(
                    "ambiguous symbol: {} matches {} indexed symbols; pass one of the qualified names",
                    req.function_name,
                    candidates.len()
                ),
                "candidates": rows,
                "rows": [],
                "total": 0,
            }));
        }
        return Ok(json!({
            "project": req.project,
            "found": false,
            "message": format!("symbol not found: {} (use search_graph first)", req.function_name),
            "rows": [],
            "total": 0,
        }));
    };
    let offsets = match req.cursor.as_deref() {
        None => 0,
        Some(cursor) => {
            let (generation, key, offset) = decode_trace_cursor(cursor)?;
            if generation != project_record.generation {
                return Err(gry_core::GryError::invalid(
                    "stale cursor: the project was reindexed; re-run the trace",
                ));
            }
            let expected = format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                start.qualified_name, req.direction, req.depth, req.mode
            );
            if key != expected {
                return Err(gry_core::GryError::invalid(
                    "cursor belongs to a different trace; re-run without a cursor",
                ));
            }
            offset
        }
    };
    let mode = req.mode.as_str();
    let edge_types = if !req.edge_types.is_empty() {
        req.edge_types.clone()
    } else {
        default_edges(mode).iter().map(|s| s.to_string()).collect()
    };
    let direction = match mode {
        "deploy" | "run" => Direction::Outbound,
        _ => Direction::parse(&req.direction).unwrap_or(Direction::Both),
    };
    let params = TraceParams {
        project: req.project.clone(),
        start_qn: start.qualified_name.clone(),
        direction,
        depth: req.depth.clamp(1, 15),
        edge_types,
        include_tests: req.include_tests,
        limit: (req.limit.clamp(1, 5000) + req.offset_or_zero()).min(5000),
        parameter_name: req.parameter_name.clone(),
    };
    let Some(trace) = store.trace(&params)? else {
        return Ok(json!({"project": req.project, "found": false, "rows": [], "total": 0}));
    };
    let mut rows: Vec<Value> = trace
        .hops
        .iter()
        .map(|hop| {
            let mut row = json!({
                "hop": hop.hop,
                "direction": hop.via_direction,
                "edge": hop.via_edge,
                "qn": hop.node.qualified_name,
                "name": hop.node.name,
                "label": hop.node.label,
                "file": hop.node.file_path,
                "lines": [hop.node.start_line, hop.node.end_line],
                "via_qn": hop.via_qn,
                "observed": hop.observed,
            });
            if hop.observed {
                row["observed_count"] = json!(hop.observed_count);
            }
            if req.risk_labels {
                row["risk"] = json!(risk_label(&hop.via_edge, &hop.node));
            }
            if req.include_evidence {
                if let Some(conf) = hop.edge_properties.get("confidence") {
                    row["confidence"] = conf.clone();
                }
                if let Some(by) = hop.edge_properties.get("resolved_by") {
                    row["resolved_by"] = by.clone();
                }
                if let Some(params) = hop.edge_properties.get("params") {
                    row["params"] = params.clone();
                }
            }
            row
        })
        .collect();
    let total = rows.len();
    let start_offset = offsets;
    if start_offset >= rows.len() {
        rows.clear();
    } else {
        rows = rows
            .into_iter()
            .skip(start_offset)
            .take(req.limit)
            .collect();
    }
    let next = start_offset + rows.len();
    Ok(json!({
        "project": req.project,
        "found": true,
        "mode": mode,
        "start": {
            "qn": trace.start.qualified_name,
            "name": trace.start.name,
            "label": trace.start.label,
            "file": trace.start.file_path,
            "lines": [trace.start.start_line, trace.start.end_line],
        },
        "rows": rows,
        "total": total,
        "returned": rows.len(),
        "depth": req.depth,
        "truncated": trace.truncated,
        "visited": trace.visited,
        "has_more": next < total,
        "next_cursor": if next < total {
            Some(encode_trace_cursor(
                &project_record.generation,
                &format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    start.qualified_name, req.direction, req.depth, req.mode
                ),
                next,
            ))
        } else {
            None
        },
        "next_offset": if next < total { Some(next) } else { None },
        "include_tests": req.include_tests,
    }))
}

fn resolve_start(
    store: &Store,
    project: &str,
    name: &str,
    mode: &str,
) -> gry_core::GryResult<Option<Node>> {
    if let Some(node) = store.resolve_node(project, name)? {
        return Ok(Some(node));
    }
    let preferred: &[&str] = match mode {
        "topology" => &["Resource", "Route"],
        "deploy" => &["Function", "Module", "File", "Process"],
        "run" => &["Resource", "Image", "Process"],
        _ => &["Function", "Method", "Class"],
    };
    let candidates: Vec<Node> = store
        .nodes_by_name(project, name)?
        .into_iter()
        .filter(|n| preferred.iter().any(|label| *label == n.label))
        .collect();
    // Only a single candidate of the mode's preferred label is unambiguous.
    // Otherwise the caller reports the candidates and asks for a qualified
    // name instead of silently tracing an arbitrary symbol.
    if candidates.len() == 1 {
        return Ok(candidates.into_iter().next());
    }
    Ok(None)
}

fn default_edges(mode: &str) -> &'static [&'static str] {
    match mode {
        "data_flow" => &["DATA_FLOWS", "CALLS"],
        "cross_service" => &[
            "CROSS_HTTP_CALLS",
            "CROSS_GRPC_CALLS",
            "CROSS_GRAPHQL_CALLS",
            "CROSS_TRPC_CALLS",
            "CROSS_ASYNC_CALLS",
            "CROSS_CHANNEL",
            "HTTP_CALLS",
            "GRPC_CALLS",
            "ASYNC_CALLS",
            "EMITS",
            "LISTENS_ON",
        ],
        "topology" => &[
            "INFRA_MAPS",
            "USAGE",
            "CONNECTS_TO",
            "DEPLOYS",
            "DEPENDS_ON",
            "HANDLES",
            "HTTP_CALLS",
            "EMITS",
        ],
        "deploy" => &[
            "DEPLOYS",
            "SPAWNS",
            "HTTP_CALLS",
            "CONFIGURES",
            "DEPENDS_ON",
        ],
        "run" => &["USAGE", "DEPENDS_ON", "SPAWNS", "CONFIGURES", "DEPLOYS"],
        _ => &["CALLS", "CALL_REFERENCE"],
    }
}

fn risk_label(edge: &str, node: &Node) -> &'static str {
    if edge.starts_with("CROSS_")
        || matches!(edge, "HTTP_CALLS" | "DEPLOYS" | "SPAWNS" | "INFRA_MAPS")
        || labels::is_infrastructure(&node.label)
    {
        "high"
    } else if matches!(edge, "CALLS" | "CALL_REFERENCE" | "USAGE" | "DATA_FLOWS") {
        "medium"
    } else {
        "low"
    }
}

fn encode_trace_cursor(generation: &str, start_qn: &str, offset: usize) -> String {
    use base64::Engine;
    let payload = serde_json::json!({"v":1,"g":generation,"s":start_qn,"o":offset});
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap_or_default())
}

fn decode_trace_cursor(cursor: &str) -> gry_core::GryResult<(String, String, usize)> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| gry_core::GryError::invalid("invalid cursor"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|_| gry_core::GryError::invalid("invalid cursor"))?;
    let generation = value
        .get("g")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))?;
    let start = value
        .get("s")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))?;
    let offset = value
        .get("o")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))? as usize;
    Ok((generation.to_string(), start.to_string(), offset))
}

impl TraceRequest {
    fn offset_or_zero(&self) -> usize {
        self.cursor
            .as_deref()
            .and_then(|c| decode_trace_cursor(c).ok())
            .map(|(_, _, offset)| offset)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    #[test]
    fn outbound_calls() {
        let store = test_store();
        let out = trace_path(
            &store,
            &TraceRequest {
                project: "p".into(),
                function_name: "p.src.main.main".into(),
                direction: "outbound".into(),
                depth: 3,
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(out["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "parseRequest"));
    }

    #[test]
    fn missing_start_is_reported() {
        let store = test_store();
        let out = trace_path(
            &store,
            &TraceRequest {
                project: "p".into(),
                function_name: "nope".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out["found"], false);
    }

    #[test]
    fn risk_and_evidence() {
        let store = test_store();
        let out = trace_path(
            &store,
            &TraceRequest {
                project: "p".into(),
                function_name: "p.src.main.main".into(),
                direction: "outbound".into(),
                risk_labels: true,
                include_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let row = out["rows"].as_array().unwrap().first().unwrap();
        assert!(row["risk"].is_string());
    }

    #[test]
    fn type_member_name_resolves_without_a_full_qn() {
        let store = test_store();
        let out = trace_path(
            &store,
            &TraceRequest {
                project: "p".into(),
                function_name: "main.main".into(),
                direction: "outbound".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out["found"], true);
        assert!(out["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "parseRequest"));
    }

    #[test]
    fn ambiguous_short_name_returns_candidates() {
        use gry_core::types::NewNode;
        use gry_store::ProjectWrite;
        let mut store = gry_store::Store::open_in_memory().unwrap();
        let nodes = vec![
            NewNode::new(
                "Function",
                "Fetch",
                "p.repository.taskRepository.Fetch",
                "repository/task.go",
                1,
                5,
            ),
            NewNode::new(
                "Function",
                "Fetch",
                "p.usecase.taskUsecase.Fetch",
                "usecase/task.go",
                1,
                5,
            ),
            NewNode::new(
                "Function",
                "Fetch",
                "p.domain.mocks.TaskRepository.Fetch",
                "domain/mocks/TaskRepository.go",
                1,
                5,
            ),
        ];
        let mut write = ProjectWrite::new("p", "/tmp/p", "full", "gen-1");
        write.nodes = &nodes;
        store.publish(&write).unwrap();
        let out = trace_path(
            &store,
            &TraceRequest {
                project: "p".into(),
                function_name: "Fetch".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out["found"], false);
        assert_eq!(out["ambiguous"], true);
        let candidates = out["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 3);
        assert!(!candidates[0]["qn"].as_str().unwrap().contains("mocks"));
    }
}
