//! Deterministic snapshot comparison by stable graph identity.

use gry_store::Store;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn compare_graphs(
    store: &Store,
    base_project: &str,
    target_project: &str,
    limit: usize,
    scan_limit: usize,
) -> gry_core::GryResult<Value> {
    let base = store.require_project(base_project)?;
    let target = store.require_project(target_project)?;
    let base_nodes = store.all_nodes(base_project)?;
    let target_nodes = store.all_nodes(target_project)?;
    let base_edges = store.all_edges(base_project)?;
    let target_edges = store.all_edges(target_project)?;

    let base_ids: HashMap<i64, &gry_core::types::Node> =
        base_nodes.iter().map(|n| (n.id, n)).collect();
    let target_ids: HashMap<i64, &gry_core::types::Node> =
        target_nodes.iter().map(|n| (n.id, n)).collect();

    let node_key = |n: &gry_core::types::Node| format!("{}\u{1f}{}", n.label, n.qualified_name);
    let base_node_set: HashMap<String, &gry_core::types::Node> =
        base_nodes.iter().map(|n| (node_key(n), n)).collect();
    let target_node_set: HashMap<String, &gry_core::types::Node> =
        target_nodes.iter().map(|n| (node_key(n), n)).collect();

    let mut nodes_added: Vec<&gry_core::types::Node> = target_node_set
        .iter()
        .filter(|(k, _)| !base_node_set.contains_key(*k))
        .map(|(_, n)| *n)
        .collect();
    let mut nodes_removed: Vec<&gry_core::types::Node> = base_node_set
        .iter()
        .filter(|(k, _)| !target_node_set.contains_key(*k))
        .map(|(_, n)| *n)
        .collect();
    nodes_added.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    nodes_removed.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

    let edge_key = |e: &gry_core::types::Edge, ids: &HashMap<i64, &gry_core::types::Node>| {
        let source = ids
            .get(&e.source_id)
            .map(|n| n.qualified_name.as_str())
            .unwrap_or("");
        let target = ids
            .get(&e.target_id)
            .map(|n| n.qualified_name.as_str())
            .unwrap_or("");
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            source,
            e.edge_type,
            target,
            e.properties
                .get("local_name")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
    };
    let base_edge_set: HashMap<String, &gry_core::types::Edge> = base_edges
        .iter()
        .take(scan_limit)
        .map(|e| (edge_key(e, &base_ids), e))
        .collect();
    let target_edge_set: HashMap<String, &gry_core::types::Edge> = target_edges
        .iter()
        .take(scan_limit)
        .map(|e| (edge_key(e, &target_ids), e))
        .collect();
    let mut edges_added: Vec<&gry_core::types::Edge> = target_edge_set
        .iter()
        .filter(|(k, _)| !base_edge_set.contains_key(*k))
        .map(|(_, e)| *e)
        .collect();
    let mut edges_removed: Vec<&gry_core::types::Edge> = base_edge_set
        .iter()
        .filter(|(k, _)| !target_edge_set.contains_key(*k))
        .map(|(_, e)| *e)
        .collect();
    edges_added.sort_by(|a, b| edge_key(a, &target_ids).cmp(&edge_key(b, &target_ids)));
    edges_removed.sort_by(|a, b| edge_key(a, &base_ids).cmp(&edge_key(b, &base_ids)));

    let node_added_total = nodes_added.len();
    let node_removed_total = nodes_removed.len();
    let edge_added_total = edges_added.len();
    let edge_removed_total = edges_removed.len();

    let node_rows = |nodes: &[&gry_core::types::Node]| {
        nodes
            .iter()
            .take(limit.max(1))
            .map(|n| {
                json!({
                    "label": n.label,
                    "qn": n.qualified_name,
                    "name": n.name,
                    "file": n.file_path,
                    "lines": [n.start_line, n.end_line],
                })
            })
            .collect::<Vec<_>>()
    };
    let edge_rows = |edges: &[&gry_core::types::Edge],
                     ids: &HashMap<i64, &gry_core::types::Node>| {
        edges
            .iter()
            .take(limit.max(1))
            .map(|e| {
                json!({
                    "source": ids.get(&e.source_id).map(|n| n.qualified_name.clone()).unwrap_or_default(),
                    "target": ids.get(&e.target_id).map(|n| n.qualified_name.clone()).unwrap_or_default(),
                    "type": e.edge_type,
                    "local_name": e.properties.get("local_name").cloned().unwrap_or(Value::Null),
                })
            })
            .collect::<Vec<_>>()
    };

    Ok(json!({
        "base_project": base_project,
        "target_project": target_project,
        "base_generation": base.generation,
        "target_generation": target.generation,
        "nodes_added": {
            "total": node_added_total,
            "returned": node_added_total.min(limit.max(1)),
            "truncated": node_added_total > limit.max(1),
            "rows": node_rows(&nodes_added),
        },
        "nodes_removed": {
            "total": node_removed_total,
            "returned": node_removed_total.min(limit.max(1)),
            "truncated": node_removed_total > limit.max(1),
            "rows": node_rows(&nodes_removed),
        },
        "edges_added": {
            "total": edge_added_total,
            "returned": edge_added_total.min(limit.max(1)),
            "truncated": edge_added_total > limit.max(1),
            "rows": edge_rows(&edges_added, &target_ids),
        },
        "edges_removed": {
            "total": edge_removed_total,
            "returned": edge_removed_total.min(limit.max(1)),
            "truncated": edge_removed_total > limit.max(1),
            "rows": edge_rows(&edges_removed, &base_ids),
        },
        "scan_truncated": base_edges.len() > scan_limit || target_edges.len() > scan_limit,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    #[test]
    fn identical_snapshots_are_empty() {
        let store = test_store();
        // Only one project exists; compare it with itself.
        let out = compare_graphs(&store, "p", "p", 20, 100).unwrap();
        assert_eq!(out["nodes_added"]["total"], 0);
        assert_eq!(out["nodes_removed"]["total"], 0);
        assert_eq!(out["edges_added"]["total"], 0);
        assert_eq!(out["edges_removed"]["total"], 0);
    }
}
