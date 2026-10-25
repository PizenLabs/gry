//! Cross-repository intelligence: match routes and channels across indexed
//! projects and record `CROSS_*` edges on the source project.

use crate::discover::IndexMode;
use crate::pipeline::{IndexReport, IndexRequest};
use gry_core::config::Config;
use gry_core::types::labels;
use gry_store::Store;
use serde_json::Value;
use serde_json::{json, Map};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

pub fn run_cross_repo(
    config: &Config,
    request: &IndexRequest,
    _cancel: &Arc<AtomicBool>,
    started: Instant,
) -> gry_core::GryResult<IndexReport> {
    let root = config.resolve_index_root(&request.repo_path)?;
    let project = request
        .name
        .as_deref()
        .map(gry_core::fqn::sanitize_project_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| gry_core::fqn::project_name_from_path(root.to_string_lossy().as_ref()));
    let store = Store::open(&config.db_path)?;
    let source = store.require_project(&project)?;
    let all = store.projects()?;
    let targets: Vec<String> =
        if request.target_projects.is_empty() || request.target_projects.iter().any(|t| t == "*") {
            all.iter()
                .filter(|p| p.name != project)
                .map(|p| p.name.clone())
                .collect()
        } else {
            request
                .target_projects
                .iter()
                .filter(|p| p.as_str() != project)
                .cloned()
                .collect()
        };
    let source_nodes = store.all_nodes(&project)?;
    let source_edges = store.all_edges(&project)?;
    let source_by_id: HashMap<i64, &gry_core::types::Node> =
        source_nodes.iter().map(|n| (n.id, n)).collect();
    let mut total = 0i64;
    let mut skipped = Vec::new();
    for target_name in &targets {
        let Some(target) = store.project(target_name)? else {
            skipped.push(json!({"name": target_name, "reason": "not_found"}));
            continue;
        };
        if target.index_version != gry_store::FORMAT_VERSION {
            skipped.push(json!({"name": target_name, "reason": "pre_768_schema", "hint": "reindex this project"}));
            continue;
        }
        let target_nodes = store.all_nodes(target_name)?;
        let routes: HashMap<String, &gry_core::types::Node> = target_nodes
            .iter()
            .filter(|n| n.label == labels::ROUTE)
            .map(|n| (n.qualified_name.clone(), n))
            .collect();
        let channels: HashMap<String, &gry_core::types::Node> = target_nodes
            .iter()
            .filter(|n| n.label == labels::CHANNEL)
            .map(|n| (n.name.clone(), n))
            .collect();
        for edge in &source_edges {
            let source_node = match source_by_id.get(&edge.source_id) {
                Some(n) => n,
                None => continue,
            };
            match edge.edge_type.as_str() {
                "HTTP_CALLS" | "GRPC_CALLS" | "ASYNC_CALLS" => {
                    let route_qn = source_by_id
                        .get(&edge.target_id)
                        .map(|n| n.qualified_name.clone());
                    let Some(route_qn) = route_qn else { continue };
                    if let Some(target_route) = routes.get(&route_qn) {
                        let cross_type = match edge.edge_type.as_str() {
                            "GRPC_CALLS" => "CROSS_GRPC_CALLS",
                            "ASYNC_CALLS" => "CROSS_ASYNC_CALLS",
                            _ => "CROSS_HTTP_CALLS",
                        };
                        let mut props = Map::new();
                        props.insert(
                            "method".to_string(),
                            Value::String(
                                edge.properties
                                    .get("method")
                                    .and_then(Value::as_str)
                                    .unwrap_or("ANY")
                                    .to_string(),
                            ),
                        );
                        props.insert(
                            "url_path".to_string(),
                            Value::String(
                                edge.properties
                                    .get("url_path")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                            ),
                        );
                        props.insert(
                            "target_project".to_string(),
                            Value::String(target_name.clone()),
                        );
                        if store.insert_cross_edge(
                            &project,
                            &source_node.qualified_name,
                            target_name,
                            &target_route.qualified_name,
                            cross_type,
                            Value::Object(props),
                        )? {
                            total += 1;
                        }
                    }
                }
                "EMITS" | "LISTENS_ON" => {
                    let name = source_by_id.get(&edge.target_id).map(|n| n.name.clone());
                    let Some(name) = name else { continue };
                    if let Some(target_channel) = channels.get(&name) {
                        let mut props = Map::new();
                        props.insert("channel".to_string(), Value::String(name));
                        props.insert(
                            "target_project".to_string(),
                            Value::String(target_name.clone()),
                        );
                        if store.insert_cross_edge(
                            &project,
                            &source_node.qualified_name,
                            target_name,
                            &target_channel.qualified_name,
                            "CROSS_CHANNEL",
                            Value::Object(props),
                        )? {
                            total += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let status = json!({
        "status": "ran",
        "last_run_at": gry_store::now_iso(),
        "outcome": "ok",
        "targets": targets,
        "projects_scanned": targets.len() - skipped.len(),
        "total_cross_edges": total,
        "skipped_projects": skipped,
    });
    store.set_meta(&format!("cross_repo:{project}"), &status.to_string())?;
    let (nodes, edges) = store.counts(&project)?;
    Ok(IndexReport {
        project: project.clone(),
        root_path: source.root_path,
        mode: IndexMode::CrossRepoIntelligence.as_str().to_string(),
        generation: source.generation,
        files: source.file_count as usize,
        source_bytes: source.source_bytes,
        nodes,
        edges,
        parsed_files: 0,
        reused_files: 0,
        deleted_files: 0,
        duration_ms: started.elapsed().as_millis(),
        unchanged: false,
        skipped: Vec::new(),
        excluded: Default::default(),
        parse_partial: Vec::new(),
        coverage: Default::default(),
        artifact: None,
        artifact_error: None,
        cross_repo: Some(status),
    })
}
