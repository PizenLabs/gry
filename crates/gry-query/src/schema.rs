//! Graph schema introspection.

use gry_store::Store;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

pub fn get_graph_schema(
    store: &Store,
    project: &str,
    diagnostics: &str,
    limit: usize,
    offset: usize,
) -> gry_core::GryResult<Value> {
    let record = store.require_project(project)?;
    let labels = store.label_counts(project)?;
    let edges = store.edge_type_counts(project)?;
    let mut patterns: Vec<(String, i64)> = {
        let mut stmt = store.connection().prepare(
            "SELECT sn.label, e.type, tn.label, COUNT(*) FROM edges e
             JOIN nodes sn ON sn.id=e.source_id JOIN nodes tn ON tn.id=e.target_id
             WHERE e.project=?1 GROUP BY sn.label, e.type, tn.label",
        )?;
        let rows = stmt.query_map([project], |row| {
            let source: String = row.get(0)?;
            let edge_type: String = row.get(1)?;
            let target: String = row.get(2)?;
            let count: i64 = row.get(3)?;
            Ok((format!("{source}-[{edge_type}]->{target}"), count))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    patterns.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let pattern_total = patterns.len();
    let patterns: Vec<Value> = patterns
        .into_iter()
        .skip(offset)
        .take(limit.max(1))
        .map(|(pattern, count)| json!({"pattern": pattern, "count": count}))
        .collect();

    let nodes = if diagnostics == "full" {
        store.all_nodes(project)?
    } else {
        Vec::new()
    };
    let all_edges = if diagnostics == "full" {
        store.all_edges(project)?
    } else {
        Vec::new()
    };
    let mut properties: BTreeMap<String, BTreeMap<String, HashSet<String>>> = BTreeMap::new();
    if diagnostics == "full" {
        for node in nodes.iter().take(20_000) {
            let entry = properties.entry(node.label.clone()).or_default();
            if let Value::Object(props) = &node.properties {
                for (k, v) in props {
                    entry.entry(k.clone()).or_default().insert(type_name(v));
                }
            }
        }
        let mut edge_props: BTreeMap<String, BTreeMap<String, HashSet<String>>> = BTreeMap::new();
        for edge in all_edges.iter().take(20_000) {
            let entry = edge_props.entry(edge.edge_type.clone()).or_default();
            if let Value::Object(props) = &edge.properties {
                for (k, v) in props {
                    entry.entry(k.clone()).or_default().insert(type_name(v));
                }
            }
        }
        for (label, props) in edge_props {
            let entry = properties.entry(format!("edge:{label}")).or_default();
            for (k, types) in props {
                entry.entry(k).or_default().extend(types);
            }
        }
    }
    let properties_json: Value = Value::Object(
        properties
            .into_iter()
            .map(|(label, props)| {
                let mut map = Map::new();
                for (key, types) in props {
                    let mut types: Vec<String> = types.into_iter().collect();
                    types.sort();
                    map.insert(key, json!(types));
                }
                (label, Value::Object(map))
            })
            .collect(),
    );

    Ok(json!({
        "project": project,
        "generation": record.generation,
        "nodes": label_counts_total(&labels),
        "edges": edge_counts_total(&edges),
        "labels": labels.iter().map(|(l,n)| json!({"label": l, "count": n})).collect::<Vec<_>>(),
        "edge_types": edges.iter().map(|(t,n)| json!({"type": t, "count": n})).collect::<Vec<_>>(),
        "relationship_patterns": patterns,
        "relationship_pattern_total": pattern_total,
        "properties": if diagnostics == "full" { properties_json } else { Value::Null },
        "diagnostics": diagnostics,
        "limit": limit,
        "offset": offset,
    }))
}

fn label_counts_total(rows: &[(String, i64)]) -> i64 {
    rows.iter().map(|(_, n)| n).sum()
}

fn edge_counts_total(rows: &[(String, i64)]) -> i64 {
    rows.iter().map(|(_, n)| n).sum()
}

fn type_name(value: &Value) -> String {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "map",
    }
    .to_string()
}
