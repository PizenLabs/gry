//! Coverage and index-status reporting.

use gry_store::Store;
use serde_json::{json, Value};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub fn check_index_coverage(
    store: &Store,
    project: &str,
    paths: &[String],
    scopes: &[String],
    path_limit: usize,
    path_offset: usize,
    scope_limit: usize,
    scope_offset: usize,
    diagnostics: &str,
) -> gry_core::GryResult<Value> {
    let record = store.require_project(project)?;
    let root = Path::new(&record.root_path);
    let hashes: std::collections::HashMap<String, gry_core::types::FileHash> = store
        .file_hashes(project)?
        .into_iter()
        .map(|h| (h.rel_path.clone(), h))
        .collect();
    let coverage = store.coverage(project)?;
    let coverage_map: std::collections::HashMap<&str, &str> = coverage
        .iter()
        .map(|c| (c.rel_path.as_str(), c.kind.as_str()))
        .collect();

    let mut path_rows = Vec::new();
    for path in paths {
        let normalized = path.trim_start_matches("./").trim_start_matches('/');
        let hash = hashes.get(normalized);
        let file_path = root.join(normalized);
        let metadata = std::fs::metadata(&file_path).ok();
        let stale = match (hash, metadata) {
            (Some(hash), Some(meta)) => {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i64)
                    .unwrap_or(0);
                mtime != hash.mtime_ns || meta.len() as i64 != hash.size
            }
            _ => false,
        };
        let (status, reason): (&str, &str) = match (hash, coverage_map.get(normalized)) {
            (_, Some(kind)) if *kind == "parse_partial" => ("partial", "parse_partial"),
            (_, Some(kind)) if *kind == "parse_unusable" => ("partial", "parse_unusable"),
            (Some(_), _) if stale => ("stale", "file_changed_since_index"),
            (Some(_), _) => ("indexed", "ok"),
            (None, Some(kind)) => ("skipped", *kind),
            (None, _) => ("missing", "not_indexed"),
        };
        let nodes = store.nodes_by_file(project, normalized)?.len();
        path_rows.push(json!({
            "path": normalized,
            "status": status,
            "reason": reason,
            "nodes": nodes,
            "fresh": !stale,
        }));
    }
    let path_total = path_rows.len();
    let path_page: Vec<Value> = path_rows
        .into_iter()
        .skip(path_offset)
        .take(path_limit.max(1))
        .collect();

    let mut scope_rows = Vec::new();
    for scope in scopes {
        let prefix = scope.trim_start_matches("./").trim_start_matches('/');
        let matching: Vec<&String> = hashes
            .keys()
            .filter(|p| prefix.is_empty() || p.starts_with(prefix))
            .collect();
        let indexed = matching.len();
        let partial = matching
            .iter()
            .filter(|p| {
                matches!(
                    coverage_map.get(p.as_str()),
                    Some(&"parse_partial") | Some(&"parse_unusable")
                )
            })
            .count();
        let skipped = coverage
            .iter()
            .filter(|c| {
                c.rel_path.starts_with(prefix)
                    && c.kind != "parse_partial"
                    && c.kind != "parse_unusable"
            })
            .count();
        scope_rows.push(json!({
            "scope": prefix,
            "files": indexed,
            "parse_partial": partial,
            "skipped": skipped,
            "status": if indexed == 0 { "unindexed" } else if partial > 0 || skipped > 0 { "partial" } else { "clean" },
        }));
    }
    let scope_total = scope_rows.len();
    let scope_page: Vec<Value> = scope_rows
        .into_iter()
        .skip(scope_offset)
        .take(scope_limit.max(1))
        .collect();

    let mut out = json!({
        "project": project,
        "generation": record.generation,
        "paths": path_page,
        "path_total": path_total,
        "path_offset": path_offset,
        "path_has_more": path_offset + path_limit.max(1) < path_total,
        "scopes": scope_page,
        "scope_total": scope_total,
        "scope_offset": scope_offset,
        "scope_has_more": scope_offset + scope_limit.max(1) < scope_total,
        "note": "Best-effort coverage: a clean result means no recorded gap, not proof of completeness.",
    });
    if diagnostics == "full" {
        out["coverage_rows"] = json!(coverage);
    }
    Ok(out)
}

pub fn index_status(
    store: &Store,
    project: &str,
    verbose: bool,
    diagnostics: &str,
) -> gry_core::GryResult<Value> {
    let record = store.require_project(project)?;
    let (nodes, edges) = store.counts(project)?;
    let coverage = store.coverage(project)?;
    let coverage_meta = store.coverage_meta(project)?;
    let labels = store.label_counts(project)?;
    let edges_types = store.edge_type_counts(project)?;
    let cross = store
        .get_meta(&format!("cross_repo:{project}"))?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({"status": "never_run"}));
    let doc_links = store
        .edge_type_counts(project)?
        .into_iter()
        .find(|(t, _)| t == "MENTIONS");
    let mut out = json!({
        "project": project,
        "indexed": true,
        "root_path": record.root_path,
        "generation": record.generation,
        "indexed_at": record.indexed_at,
        "mode": record.mode,
        "files": record.file_count,
        "source_bytes": record.source_bytes,
        "nodes": nodes,
        "edges": edges,
        "index_version": record.index_version,
        "watch": {
            "enabled": false,
            "strategy": "none",
            "note": "gry does not run a background daemon; run `gry watch` or re-index explicitly.",
        },
        "coverage": {
            "gaps": coverage.len(),
            "meta": coverage_meta,
        },
        "doc_links": dd_link(doc_links),
        "cross_repo": cross,
        "labels": labels.iter().map(|(l,n)| json!({"label":l,"count":n})).collect::<Vec<_>>(),
        "edge_types": edges_types.iter().map(|(t,n)| json!({"type":t,"count":n})).collect::<Vec<_>>(),
    });
    if diagnostics != "none" {
        let sample = if diagnostics == "summary" { 5 } else { 500 };
        out["coverage"]["rows"] = json!(coverage.iter().take(sample).collect::<Vec<_>>());
        out["coverage"]["truncated"] = json!(coverage.len() > sample);
    }
    if verbose {
        let canonical = Path::new(&record.root_path).canonicalize().ok();
        out["git"] = json!({
            "root_path": record.root_path,
            "canonical_root": canonical.map(|p| p.to_string_lossy().to_string()),
            "is_repository": Path::new(&record.root_path).join(".git").exists(),
        });
    }
    out["note"] = json!("Best-effort status; verify cited paths with check_index_coverage.");
    Ok(out)
}

fn dd_link(count: Option<(String, i64)>) -> Value {
    json!({
        "mentions": count.map(|(_, n)| n).unwrap_or(0),
        "unresolved_reported": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    #[test]
    fn coverage_reports_missing_and_indexed() {
        let store = test_store();
        // test_store has no real root; use a path with no FS metadata.
        let out = check_index_coverage(
            &store,
            "p",
            &["src/lib.rs".to_string(), "nope.rs".to_string()],
            &["src".to_string()],
            20,
            0,
            20,
            0,
            "none",
        )
        .unwrap();
        let rows = out["paths"].as_array().unwrap();
        assert_eq!(rows[0]["status"], "indexed");
        assert_eq!(rows[1]["status"], "missing");
        assert_eq!(out["scopes"][0]["files"], 2);
    }

    #[test]
    fn status_never_run_cross_repo() {
        let store = test_store();
        let out = index_status(&store, "p", false, "none").unwrap();
        assert_eq!(out["cross_repo"]["status"], "never_run");
        assert_eq!(out["nodes"], 6);
    }
}
