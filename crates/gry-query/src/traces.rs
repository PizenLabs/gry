//! Runtime trace ingestion: observed calls and journeys keyed by qualified name.

use gry_store::Store;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub struct IngestRequest {
    pub project: String,
    pub label: Option<String>,
    pub traces: Vec<Value>,
}

pub fn ingest_traces(store: &Store, req: &IngestRequest) -> gry_core::GryResult<Value> {
    store.require_project(&req.project)?;
    let label = req
        .label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| "unlabeled".to_string());
    if label.len() > 200 {
        return Err(gry_core::GryError::invalid(
            "label must be at most 200 bytes",
        ));
    }
    let nodes = store.all_nodes(&req.project)?;
    let by_qn: HashMap<String, String> = nodes
        .iter()
        .map(|n| (n.qualified_name.clone(), n.qualified_name.clone()))
        .collect();
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for node in &nodes {
        by_name
            .entry(node.name.clone())
            .or_default()
            .push(node.qualified_name.clone());
    }
    let resolve = |name: &str| -> Option<String> {
        if let Some(qn) = by_qn.get(name) {
            return Some(qn.clone());
        }
        let candidates = by_name.get(name)?;
        if candidates.len() == 1 {
            candidates.first().cloned()
        } else {
            None
        }
    };
    let mut stored_calls = 0i64;
    let mut stored_paths = 0i64;
    let mut resolved_pairs = 0i64;
    let mut unresolved: Vec<String> = Vec::new();
    let mut unresolved_set = HashSet::new();
    let mut paths_seen = 0i64;
    for trace in &req.traces {
        let count = trace
            .get("count")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .clamp(1, 1_000_000_000);
        let path = trace
            .get("path")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if path.len() >= 2 {
            paths_seen += 1;
            if path.len() > 256 {
                return Err(gry_core::GryError::invalid(
                    "trace path must have 2..256 entries",
                ));
            }
            let resolved: Vec<Option<String>> = path.iter().map(|n| resolve(n)).collect();
            for (i, r) in resolved.iter().enumerate() {
                if r.is_none() {
                    let name = &path[i];
                    if unresolved_set.insert(name.clone()) && unresolved.len() < 10 {
                        unresolved.push(name.clone());
                    }
                }
            }
            if resolved.iter().all(Option::is_some) {
                for pair in resolved.windows(2) {
                    if let [Some(a), Some(b)] = pair {
                        store.upsert_observed_call(&req.project, a, b, &label, count)?;
                        stored_calls += 1;
                        resolved_pairs += 1;
                    }
                }
                if let Ok(json) = serde_json::to_string(&path) {
                    store.upsert_observed_path(&req.project, &label, &json, count)?;
                    stored_paths += 1;
                }
            }
            continue;
        }
        let caller = trace.get("caller").and_then(Value::as_str);
        let callee = trace.get("callee").and_then(Value::as_str);
        let (Some(caller), Some(callee)) = (caller, callee) else {
            return Err(gry_core::GryError::invalid(
                "each trace needs a path or caller/callee pair",
            ));
        };
        match (resolve(caller), resolve(callee)) {
            (Some(c), Some(d)) => {
                store.upsert_observed_call(&req.project, &c, &d, &label, count)?;
                stored_calls += 1;
                resolved_pairs += 1;
            }
            (c, d) => {
                if c.is_none() && unresolved_set.insert(caller.to_string()) && unresolved.len() < 10
                {
                    unresolved.push(caller.to_string());
                }
                if d.is_none() && unresolved_set.insert(callee.to_string()) && unresolved.len() < 10
                {
                    unresolved.push(callee.to_string());
                }
            }
        }
    }
    store.prune_observed_labels(&req.project, 32)?;
    Ok(json!({
        "project": req.project,
        "label": label,
        "stored_calls": stored_calls,
        "stored_paths": stored_paths,
        "resolved_pairs": resolved_pairs,
        "paths": paths_seen,
        "unresolved": unresolved,
        "unresolved_total": unresolved_set.len(),
        "note": "Only pairs whose endpoints are indexed symbols are stored; names are matched exactly or by a unique short name.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    #[test]
    fn ingests_pairs_and_paths() {
        let store = test_store();
        let out = ingest_traces(
            &store,
            &IngestRequest {
                project: "p".into(),
                label: Some("pytest".into()),
                traces: vec![
                    json!({"path": ["p.src.main.main", "p.src.lib.parseRequest"], "count": 2}),
                    json!({"caller": "p.src.main.main", "callee": "p.src.lib.saveOrder"}),
                    json!({"caller": "nope", "callee": "p.src.lib.saveOrder"}),
                ],
            },
        )
        .unwrap();
        assert_eq!(out["stored_calls"], 2);
        assert_eq!(out["stored_paths"], 1);
        assert!(out["unresolved"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "nope"));
        let calls = store.observed_calls("p", Some("pytest")).unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().any(|(_, _, _, c)| *c == 2));
    }

    #[test]
    fn invalid_trace_is_rejected() {
        let store = test_store();
        let err = ingest_traces(
            &store,
            &IngestRequest {
                project: "p".into(),
                label: None,
                traces: vec![json!({"caller": "a"})],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("caller/callee"));
    }
}
