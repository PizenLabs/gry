//! Structural, BM25 and semantic search, plus graph-augmented text search.

use gry_core::nomic;
use gry_core::tokenize::split_text;
use gry_core::types::{labels, Node};
use gry_store::search::{SearchHit, SearchParams};
use gry_store::Store;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct GraphSearchRequest {
    pub project: String,
    pub query: Option<String>,
    pub label: Option<String>,
    pub name_pattern: Option<String>,
    pub qn_pattern: Option<String>,
    pub file_pattern: Option<String>,
    pub relationship: Option<String>,
    pub min_degree: Option<i64>,
    pub max_degree: Option<i64>,
    pub exclude_entry_points: bool,
    pub include_connected: bool,
    pub kind: Option<String>,
    pub namespace: Option<String>,
    pub semantic_query: Option<Vec<String>>,
    pub semantic_limit: usize,
    pub semantic_offset: usize,
    pub limit: usize,
    pub offset: usize,
    pub fields: Vec<String>,
    pub detail_ids: bool,
}

pub fn search_graph(store: &Store, req: &GraphSearchRequest) -> gry_core::GryResult<Value> {
    // Match the other readers: an unknown project is an error, not an empty
    // result. Silent zeroes make an unindexed repository look like a repository
    // with no symbols.
    store.require_project(&req.project)?;
    let mut structural = None;
    let mut total = 0i64;
    if let Some(query) = req.query.as_deref().filter(|q| !q.is_empty()) {
        if req.semantic_query.is_some() {
            return Err(gry_core::GryError::invalid(
                "query and semantic_query are mutually exclusive — use query for BM25 ranking or semantic_query for vector ranking",
            ));
        }
        structural = store.bm25(
            &req.project,
            query,
            req.file_pattern.as_deref(),
            req.label.as_deref(),
            req.kind.as_deref(),
            req.namespace.as_deref(),
            req.limit.max(1),
            req.offset,
        )?;
        if let Some(out) = &structural {
            total = out.total;
        }
    }
    let has_filters = req.label.is_some()
        || req.name_pattern.is_some()
        || req.qn_pattern.is_some()
        || req.file_pattern.is_some()
        || req.relationship.is_some()
        || req.exclude_entry_points
        || req.min_degree.is_some()
        || req.max_degree.is_some();
    if structural.is_none() && !(req.semantic_query.is_some() && !has_filters) {
        let params = SearchParams {
            project: req.project.clone(),
            label: req.label.clone(),
            name_pattern: req.name_pattern.clone(),
            qn_pattern: req.qn_pattern.clone(),
            file_pattern: req.file_pattern.clone(),
            relationship: req.relationship.clone(),
            min_degree: req.min_degree,
            max_degree: req.max_degree,
            exclude_entry_points: req.exclude_entry_points,
            include_connected: req.include_connected,
            kind: req.kind.clone(),
            namespace: req.namespace.clone(),
            limit: req.limit.max(1),
            offset: req.offset,
            case_sensitive: false,
        };
        let out = store.search(&params)?;
        total = out.total;
        structural = Some(out);
    }

    let semantic = if let Some(queries) = &req.semantic_query {
        Some(semantic_search(
            store,
            &req.project,
            queries,
            req.semantic_limit,
            req.semantic_offset,
        )?)
    } else {
        None
    };

    let mut results = Vec::new();
    if let Some(out) = &structural {
        for hit in &out.hits {
            results.push(render_hit(hit, req));
        }
    }
    let semantic_json = semantic.map(|page| {
        json!({
            "total": page.total,
            "offset": page.offset,
            "limit": page.limit,
            "has_more": page.offset + page.rows.len() < page.total,
            "next_offset": if page.offset + page.rows.len() < page.total { Some(page.offset + page.rows.len()) } else { None },
            "rows": page.rows,
        })
    });
    let returned = results.len();
    Ok(json!({
        "project": req.project,
        "total": total,
        "returned": returned,
        "offset": req.offset,
        "has_more": req.offset + returned < total as usize,
        "next_offset": if req.offset + returned < total as usize { Some(req.offset + returned) } else { None },
        "results": results,
        "semantic": semantic_json,
    }))
}

fn render_hit(hit: &SearchHit, req: &GraphSearchRequest) -> Value {
    if req.detail_ids {
        return json!({
            "qn": hit.node.qualified_name,
            "name": hit.node.name,
        });
    }
    let mut map = Map::new();
    let wants = |field: &str| {
        req.fields.is_empty()
            || req.fields.iter().any(|f| f == field)
            || matches!(field, "qn" | "name" | "label" | "file" | "lines")
    };
    if wants("qn") {
        map.insert("qn".into(), Value::String(hit.node.qualified_name.clone()));
    }
    if wants("name") {
        map.insert("name".into(), Value::String(hit.node.name.clone()));
    }
    if wants("label") {
        map.insert("label".into(), Value::String(hit.node.label.clone()));
    }
    if wants("file") {
        map.insert("file".into(), Value::String(hit.node.file_path.clone()));
    }
    if wants("lines") {
        map.insert(
            "lines".into(),
            json!([hit.node.start_line, hit.node.end_line]),
        );
    }
    if wants("degree") || wants("in") || wants("out") {
        map.insert("in".into(), json!(hit.in_degree));
        map.insert("out".into(), json!(hit.out_degree));
    }
    if wants("docstring") {
        if let Some(doc) = hit.node.property_str("docstring") {
            map.insert("docstring".into(), Value::String(doc.to_string()));
        }
    }
    if wants("kind") {
        if let Some(kind) = hit.node.property_str("kind") {
            map.insert("kind".into(), Value::String(kind.to_string()));
        }
    }
    if wants("namespace") {
        if let Some(ns) = hit.node.property_str("namespace") {
            map.insert("namespace".into(), Value::String(ns.to_string()));
        }
    }
    if req.include_connected && !hit.connected_names.is_empty() {
        map.insert("connected".into(), json!(hit.connected_names));
    }
    Value::Object(map)
}

struct SemanticPage {
    total: usize,
    offset: usize,
    limit: usize,
    rows: Vec<Value>,
}

fn semantic_search(
    store: &Store,
    project: &str,
    queries: &[String],
    limit: usize,
    offset: usize,
) -> gry_core::GryResult<SemanticPage> {
    if queries.is_empty() {
        return Err(gry_core::GryError::invalid(
            "semantic_query must be a non-empty array of keyword strings",
        ));
    }
    let nodes = store.all_nodes(project)?;
    let by_id: HashMap<i64, &Node> = nodes.iter().map(|n| (n.id, n)).collect();
    let vectors = store.vectors(project)?;
    let mut query_vectors: Vec<Vec<i8>> = Vec::new();
    for query in queries {
        if let Some(vector) = nomic::embed_text(query) {
            query_vectors.push(vector);
        } else {
            // Fall back to identifier tokens in case the whole phrase is not
            // in the static vocabulary.
            let tokens = split_text(query);
            if let Some(vector) = nomic::embed_tokens(tokens.iter().map(String::as_str)) {
                query_vectors.push(vector);
            }
        }
    }
    if query_vectors.is_empty() {
        return Ok(SemanticPage {
            total: 0,
            offset,
            limit,
            rows: Vec::new(),
        });
    }
    let mut scored: Vec<(f32, &Node)> = Vec::new();
    for (id, vector) in &vectors {
        let Some(node) = by_id.get(id) else { continue };
        let mut best = 0.0f32;
        for q in &query_vectors {
            best = best.max(nomic::cosine_i8(q, vector));
        }
        if best > 0.15 {
            scored.push((best, node));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.qualified_name.cmp(&b.1.qualified_name))
    });
    let total = scored.len();
    let rows: Vec<Value> = scored
        .into_iter()
        .skip(offset)
        .take(limit.max(1))
        .map(|(score, node)| {
            json!({
                "qn": node.qualified_name,
                "name": node.name,
                "label": node.label,
                "file": node.file_path,
                "lines": [node.start_line, node.end_line],
                "score": (score * 1000.0).round() / 1000.0,
            })
        })
        .collect();
    Ok(SemanticPage {
        total,
        offset,
        limit,
        rows,
    })
}

#[derive(Debug, Clone)]
pub struct SearchCodeRequest {
    pub project: String,
    pub pattern: String,
    pub file_pattern: Option<String>,
    pub path_filter: Option<String>,
    pub mode: String,
    pub context: usize,
    pub regex: bool,
    pub debug: bool,
    pub result_limit: usize,
    pub result_offset: usize,
    pub raw_limit: usize,
    pub raw_offset: usize,
    pub directory_limit: usize,
    pub directory_offset: usize,
    pub match_limit: usize,
    pub source_max_lines: usize,
    pub cursor: Option<String>,
}

impl Default for SearchCodeRequest {
    fn default() -> Self {
        Self {
            project: String::new(),
            pattern: String::new(),
            file_pattern: None,
            path_filter: None,
            mode: "compact".to_string(),
            context: 0,
            regex: false,
            debug: false,
            result_limit: 10,
            result_offset: 0,
            raw_limit: 5,
            raw_offset: 0,
            directory_limit: 20,
            directory_offset: 0,
            match_limit: 8,
            source_max_lines: 20,
            cursor: None,
        }
    }
}

pub fn search_code(store: &Store, req: &SearchCodeRequest) -> gry_core::GryResult<Value> {
    let started = std::time::Instant::now();
    let project = store.require_project(&req.project)?;
    let root = Path::new(&project.root_path);
    let cursor_key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        req.pattern,
        req.mode,
        req.regex,
        req.file_pattern.as_deref().unwrap_or("")
    );
    let result_offset = match req.cursor.as_deref() {
        None => req.result_offset,
        Some(cursor) => {
            let (generation, key, offset) = decode_search_cursor(cursor)?;
            if generation != project.generation {
                return Err(gry_core::GryError::invalid(
                    "stale cursor: the project was reindexed; re-run the search",
                ));
            }
            if key != cursor_key {
                return Err(gry_core::GryError::invalid(
                    "cursor belongs to a different search; re-run without a cursor",
                ));
            }
            offset
        }
    };
    let matcher = if req.regex {
        Regex::new(&req.pattern).map_err(|e| {
            gry_core::GryError::invalid(format!("invalid regex {:?}: {e}", req.pattern))
        })?
    } else {
        Regex::new(&regex::escape(&req.pattern))
            .map_err(|e| gry_core::GryError::invalid(format!("invalid literal pattern: {e}")))?
    };
    let file_glob = req
        .file_pattern
        .as_deref()
        .map(globset::Glob::new)
        .transpose()
        .map_err(|e| gry_core::GryError::invalid(format!("invalid file_pattern: {e}")))?;
    let files = store.file_hashes(&req.project)?;
    let scope_start = std::time::Instant::now();
    let mut file_hits = Vec::new();
    let mut raw_rows = Vec::new();
    let mut directories: HashMap<String, usize> = HashMap::new();
    let mut total_matches = 0usize;
    for file in &files {
        if let Some(glob) = &file_glob {
            if !glob.compile_matcher().is_match(&file.rel_path) {
                continue;
            }
        }
        if let Some(filter) = &req.path_filter {
            if !file.rel_path.contains(filter) {
                continue;
            }
        }
        let path = root.join(&file.rel_path);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut matches = Vec::new();
        let all_lines: Vec<&str> = text.lines().collect();
        for (idx, line) in all_lines.iter().enumerate() {
            if matcher.is_match(line) {
                total_matches += 1;
                if raw_rows.len() < req.raw_offset + req.raw_limit.max(1) {
                    raw_rows.push(json!({
                        "file": file.rel_path,
                        "line": idx + 1,
                        "text": line.chars().take(400).collect::<String>(),
                    }));
                }
                if matches.len() < req.match_limit {
                    matches.push((idx, line.to_string()));
                }
            }
        }
        if matches.is_empty() {
            continue;
        }
        let dir = file
            .rel_path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_else(|| ".".to_string());
        *directories.entry(dir).or_insert(0) += 1;
        let symbols = store.nodes_by_file(&req.project, &file.rel_path)?;
        let compact = matches
            .iter()
            .map(|(idx, line)| {
                let symbol = symbols
                    .iter()
                    .filter(|n| n.start_line as usize <= *idx + 1 && n.end_line as usize > *idx)
                    .max_by_key(|n| n.start_line)
                    .map(|n| {
                        json!({
                            "qn": n.qualified_name,
                            "label": n.label,
                            "name": n.name,
                        })
                    });
                let (start, preview) = if req.context > 0 {
                    let start = idx.saturating_sub(req.context);
                    let end = (idx + req.context + 1).min(all_lines.len());
                    (start + 1, all_lines[start..end].join("\n"))
                } else {
                    (idx + 1, line.chars().take(400).collect::<String>())
                };
                json!({
                    "line": idx + 1,
                    "context_start": start,
                    "text": preview,
                    "symbol": symbol,
                })
            })
            .collect::<Vec<_>>();
        let source = if req.mode == "full" {
            Some(
                text.lines()
                    .take(req.source_max_lines.max(1))
                    .enumerate()
                    .map(|(i, l)| format!("{}\t{}", i + 1, l))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else {
            None
        };
        file_hits.push(json!({
            "file": file.rel_path,
            "matches": compact,
            "match_count": matches.len(),
            "symbols": symbols.iter()
                .filter(|n| !matches!(n.label.as_str(), "File"|"Folder"|"Project"|"Package"))
                .take(8)
                .map(|n| json!({"qn": n.qualified_name, "label": n.label, "name": n.name}))
                .collect::<Vec<_>>(),
            "source": source,
        }));
    }
    let scope_ms = scope_start.elapsed().as_millis();
    let scan_ms = started.elapsed().as_millis();
    let total_files = file_hits.len();
    let file_hits: Vec<Value> = if req.mode == "files" {
        file_hits
            .into_iter()
            .map(|hit| {
                json!({
                    "file": hit.get("file").cloned().unwrap_or(Value::Null),
                    "match_count": hit.get("match_count").cloned().unwrap_or(Value::Null),
                })
            })
            .collect()
    } else {
        file_hits
    };
    let result_page: Vec<Value> = file_hits
        .into_iter()
        .skip(result_offset)
        .take(req.result_limit.max(1))
        .collect();
    let raw_page: Vec<Value> = raw_rows
        .into_iter()
        .skip(req.raw_offset)
        .take(req.raw_limit.max(1))
        .collect();
    let mut dirs: Vec<Value> = directories
        .into_iter()
        .map(|(path, files)| json!({"path": path, "files": files}))
        .collect();
    dirs.sort_by(|a, b| {
        b.get("files")
            .and_then(Value::as_u64)
            .cmp(&a.get("files").and_then(Value::as_u64))
            .then(
                a.get("path")
                    .and_then(Value::as_str)
                    .cmp(&b.get("path").and_then(Value::as_str)),
            )
    });
    let dirs: Vec<Value> = dirs
        .into_iter()
        .skip(req.directory_offset)
        .take(req.directory_limit.max(1))
        .collect();
    let mut out = json!({
        "project": req.project,
        "pattern": req.pattern,
        "mode": req.mode,
        "total_files": total_files,
        "total_matches": total_matches,
        "result_offset": result_offset,
        "result_returned": result_page.len(),
        "has_more": result_offset + result_page.len() < total_files,
        "next_offset": if result_offset + result_page.len() < total_files { Some(result_offset + result_page.len()) } else { None },
        "next_cursor": if result_offset + result_page.len() < total_files {
            Some(encode_search_cursor(&project.generation, &cursor_key, result_offset + result_page.len()))
        } else {
            None
        },
        "results": result_page,
        "raw_total": total_matches,
        "raw": raw_page,
        "directories": dirs,
        "scope": "indexed_files",
    });
    if req.debug {
        out["debug"] = json!({
            "scope_ms": scope_ms,
            "scan_ms": scan_ms,
            "enrich_ms": started.elapsed().as_millis().saturating_sub(scan_ms),
        });
    }
    Ok(out)
}

// ── Snippets and outlines ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SnippetRequest {
    pub project: String,
    pub qualified_name: String,
    pub include_neighbors: bool,
    pub source_mode: String,
    pub member_limit: usize,
    pub member_offset: usize,
    pub start_line: Option<i32>,
    pub max_lines: Option<usize>,
}

pub fn get_code_snippet(store: &Store, req: &SnippetRequest) -> gry_core::GryResult<Value> {
    let project = store.require_project(&req.project)?;
    let node = store
        .resolve_node(&req.project, &req.qualified_name)?
        .ok_or_else(|| {
            gry_core::GryError::invalid(format!(
                "symbol not found: {} (use search_graph to find its qualified name)",
                req.qualified_name
            ))
        })?;
    let root = Path::new(&project.root_path);
    let path = root.join(&node.file_path);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = req.start_line.unwrap_or(node.start_line).max(1) as usize;
    let end = node.end_line.max(start as i32) as usize;
    let mode = if req.source_mode == "auto" {
        if labels::is_container(&node.label) && end.saturating_sub(start) >= 200 {
            "outline"
        } else {
            "full"
        }
    } else {
        req.source_mode.as_str()
    };
    let members = store.nodes_by_file(&req.project, &node.file_path)?;
    let children: Vec<&Node> = members
        .iter()
        .filter(|n| n.id != node.id)
        .filter(|n| n.start_line >= node.start_line && n.end_line <= node.end_line)
        .filter(|n| {
            !matches!(
                n.label.as_str(),
                "File" | "Folder" | "Project" | "Package" | "Module" | "Section"
            )
        })
        .collect();
    let (source, outline) = if mode == "outline" {
        let page: Vec<Value> = children
            .iter()
            .skip(req.member_offset)
            .take(req.member_limit.max(1))
            .map(|n| {
                json!({
                    "qn": n.qualified_name,
                    "name": n.name,
                    "label": n.label,
                    "lines": [n.start_line, n.end_line],
                })
            })
            .collect();
        (
            Value::Null,
            Some(json!({
                "total": children.len(),
                "offset": req.member_offset,
                "limit": req.member_limit,
                "has_more": req.member_offset + page.len() < children.len(),
                "members": page,
            })),
        )
    } else {
        let max_lines = req.max_lines.unwrap_or(usize::MAX);
        let page: Vec<String> = lines
            .iter()
            .skip(start.saturating_sub(1))
            .take(end.saturating_sub(start) + 1)
            .take(max_lines)
            .enumerate()
            .map(|(i, l)| format!("{}\t{}", start + i, l))
            .collect();
        (Value::String(page.join("\n")), None)
    };
    let mut out = json!({
        "project": req.project,
        "qn": node.qualified_name,
        "name": node.name,
        "label": node.label,
        "file": node.file_path,
        "lines": [node.start_line, node.end_line],
        "source_mode": mode,
        "source": source,
        "outline": outline,
        "total_lines": end.saturating_sub(start) + 1,
    });
    if req.include_neighbors {
        let edges = store.edges_for_nodes(
            &[node.id],
            &[
                "CALLS".to_string(),
                "USAGE".to_string(),
                "INHERITS".to_string(),
                "IMPLEMENTS".to_string(),
            ],
            "both",
        )?;
        let mut callers = Vec::new();
        let mut callees = Vec::new();
        for edge in &edges {
            if edge.target_id == node.id {
                if let Some(n) = store.node_by_id(&req.project, edge.source_id)? {
                    callers.push(n.qualified_name);
                }
            } else if let Some(n) = store.node_by_id(&req.project, edge.target_id)? {
                callees.push(n.qualified_name);
            }
        }
        callers.sort();
        callers.dedup();
        callees.sort();
        callees.dedup();
        out["callers"] = json!(callers);
        out["callees"] = json!(callees);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct OutlineRequest {
    pub project: String,
    pub file_path: String,
    pub labels: Vec<String>,
    pub limit: usize,
    pub offset: usize,
}

pub fn get_file_outline(store: &Store, req: &OutlineRequest) -> gry_core::GryResult<Value> {
    store.require_project(&req.project)?;
    let mut nodes = store.nodes_by_file(&req.project, &req.file_path)?;
    nodes.retain(|n| {
        !matches!(
            n.label.as_str(),
            "File" | "Folder" | "Project" | "Package" | "Module" | "Section"
        )
    });
    if !req.labels.is_empty() {
        nodes.retain(|n| req.labels.contains(&n.label));
    }
    nodes.sort_by(|a, b| {
        a.start_line
            .cmp(&b.start_line)
            .then(a.end_line.cmp(&b.end_line))
            .then(a.label.cmp(&b.label))
    });
    let total = nodes.len();
    let rows: Vec<Value> = nodes
        .into_iter()
        .skip(req.offset)
        .take(req.limit.max(1))
        .map(|n| {
            json!({
                "qn": n.qualified_name,
                "name": n.name,
                "label": n.label,
                "lines": [n.start_line, n.end_line],
            })
        })
        .collect();
    Ok(json!({
        "project": req.project,
        "file": req.file_path,
        "total": total,
        "offset": req.offset,
        "limit": req.limit,
        "has_more": req.offset + rows.len() < total,
        "rows": rows,
    }))
}

fn encode_search_cursor(generation: &str, pattern: &str, offset: usize) -> String {
    use base64::Engine;
    let payload = serde_json::json!({"v":1,"g":generation,"p":pattern,"o":offset});
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap_or_default())
}

fn decode_search_cursor(cursor: &str) -> gry_core::GryResult<(String, String, usize)> {
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
    let pattern = value
        .get("p")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))?;
    let offset = value
        .get("o")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))? as usize;
    Ok((generation.to_string(), pattern.to_string(), offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    #[test]
    fn structural_search_and_paging() {
        let store = test_store();
        let req = GraphSearchRequest {
            project: "p".into(),
            label: Some("Function".into()),
            limit: 2,
            ..Default::default()
        };
        let out = search_graph(&store, &req).unwrap();
        assert!(out["total"].as_i64().unwrap() >= 4);
        assert!(out["results"].as_array().unwrap().len() <= 2);
    }

    #[test]
    fn unknown_project_is_an_error() {
        let store = test_store();
        let req = GraphSearchRequest {
            project: "not-indexed".into(),
            ..Default::default()
        };
        let err = search_graph(&store, &req).unwrap_err();
        assert!(
            err.to_string().contains("project not found or not indexed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn bm25_and_semantic() {
        let store = test_store();
        let req = GraphSearchRequest {
            project: "p".into(),
            query: Some("parse request".into()),
            limit: 10,
            ..Default::default()
        };
        let out = search_graph(&store, &req).unwrap();
        assert!(out["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "parseRequest"));

        let req = GraphSearchRequest {
            project: "p".into(),
            semantic_query: Some(vec!["parse".into()]),
            semantic_limit: 10,
            limit: 10,
            ..Default::default()
        };
        let out = search_graph(&store, &req).unwrap();
        // No vectors in the synthetic store: the page exists with total 0.
        assert!(out["semantic"].is_object());
    }

    #[test]
    fn outlines_filter_and_page() {
        let store = test_store();
        let out = get_file_outline(
            &store,
            &OutlineRequest {
                project: "p".into(),
                file_path: "src/lib.rs".into(),
                labels: vec!["Function".into()],
                limit: 1,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(out["total"].as_i64().unwrap(), 2);
        assert_eq!(out["rows"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn snippet_outlines_large_containers() {
        let store = test_store();
        let req = SnippetRequest {
            project: "p".into(),
            qualified_name: "p.src.main".into(),
            include_neighbors: true,
            source_mode: "outline".into(),
            member_limit: 10,
            member_offset: 0,
            start_line: None,
            max_lines: None,
        };
        let out = get_code_snippet(&store, &req).unwrap();
        assert!(out["outline"].is_object());
    }
}
