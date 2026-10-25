//! Structural and full-text search over the graph.

use crate::error::StoreResult;
use crate::{row_to_node, Store};
use gry_core::tokenize::split_text;
use gry_core::types::labels;
use gry_core::types::Node;
use regex::Regex;
use rusqlite::params;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub project: String,
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
    pub limit: usize,
    pub offset: usize,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub node: Node,
    pub in_degree: i64,
    pub out_degree: i64,
    pub connected_names: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOutput {
    pub hits: Vec<SearchHit>,
    pub total: i64,
}

const DEGREE_TYPES: &[&str] = &["CALLS", "USAGE", "CALL_REFERENCE", "INHERITS", "IMPLEMENTS"];

impl Store {
    pub fn search(&self, params: &SearchParams) -> StoreResult<SearchOutput> {
        let name_re = build_regex(params.name_pattern.as_deref(), params.case_sensitive)?;
        let qn_re = build_regex(params.qn_pattern.as_deref(), params.case_sensitive)?;
        let file_glob = params
            .file_pattern
            .as_deref()
            .map(globset::Glob::new)
            .transpose()
            .map_err(|e| gry_core::GryError::invalid(format!("invalid file_pattern: {e}")))?;

        let mut sql = String::from(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties \
             FROM nodes WHERE project=?1",
        );
        let mut values: Vec<Value> = vec![Value::String(params.project.clone())];
        if let Some(label) = &params.label {
            sql.push_str(" AND label=?2");
            values.push(Value::String(label.clone()));
        }
        sql.push_str(" ORDER BY name, id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rusqlite_values: Vec<&dyn rusqlite::ToSql> = values
            .iter()
            .map(|v| match v {
                Value::String(s) => s as &dyn rusqlite::ToSql,
                _ => unreachable!(),
            })
            .collect();
        let rows = stmt.query_map(rusqlite_values.as_slice(), row_to_node)?;
        let candidates: Vec<Node> = rows.collect::<Result<Vec<_>, _>>()?;

        // Degree maps over the graph, computed once rather than per candidate.
        let (in_map, out_map) = self.degree_maps(&params.project)?;

        let mut filtered: Vec<(Node, i64, i64)> = Vec::new();
        for node in candidates {
            if let Some(re) = &name_re {
                if !re.is_match(&node.name) {
                    continue;
                }
            }
            if let Some(re) = &qn_re {
                if !re.is_match(&node.qualified_name) {
                    continue;
                }
            }
            if let Some(glob) = &file_glob {
                if !glob.compile_matcher().is_match(&node.file_path) {
                    continue;
                }
            }
            if let Some(kind) = &params.kind {
                if node.property_str("kind") != Some(kind.as_str()) {
                    continue;
                }
            }
            if let Some(ns) = &params.namespace {
                let node_ns = node
                    .property_str("namespace")
                    .or_else(|| node.property_str("effective_namespace"))
                    .unwrap_or("");
                if node_ns != ns {
                    continue;
                }
            }
            let in_deg = *in_map.get(&node.id).unwrap_or(&0);
            let out_deg = *out_map.get(&node.id).unwrap_or(&0);
            if let Some(min) = params.min_degree {
                if in_deg + out_deg < min {
                    continue;
                }
            }
            if let Some(max) = params.max_degree {
                if in_deg + out_deg > max {
                    continue;
                }
            }
            if params.exclude_entry_points && is_entry_point(&node, in_deg, out_deg) {
                continue;
            }
            if let Some(rel) = &params.relationship {
                if !self.has_relationship(node.id, rel)? {
                    continue;
                }
            }
            filtered.push((node, in_deg, out_deg));
        }

        let total = filtered.len() as i64;
        let start = params.offset.min(filtered.len());
        let end = (start + params.limit.max(1)).min(filtered.len());
        let mut hits = Vec::with_capacity(end.saturating_sub(start));
        for (node, in_deg, out_deg) in filtered.drain(start..end) {
            let connected_names = if params.include_connected {
                self.connected_names(node.id, params.relationship.as_deref(), 20)?
            } else {
                Vec::new()
            };
            hits.push(SearchHit {
                node,
                in_degree: in_deg,
                out_degree: out_deg,
                connected_names,
            });
        }
        Ok(SearchOutput { hits, total })
    }

    fn degree_maps(&self, project: &str) -> StoreResult<(HashMap<i64, i64>, HashMap<i64, i64>)> {
        let ph = DEGREE_TYPES
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let mut in_map = HashMap::new();
        let sql_in = format!(
            "SELECT target_id,COUNT(*) FROM edges WHERE project=?1 AND type IN ({ph}) GROUP BY target_id"
        );
        let mut stmt = self.conn.prepare(&sql_in)?;
        let mut vals: Vec<&dyn rusqlite::ToSql> = vec![&project];
        for t in DEGREE_TYPES {
            vals.push(t);
        }
        let rows = stmt.query_map(vals.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (id, count) = row?;
            in_map.insert(id, count);
        }
        let sql_out = format!(
            "SELECT source_id,COUNT(*) FROM edges WHERE project=?1 AND type IN ({ph}) GROUP BY source_id"
        );
        let mut stmt = self.conn.prepare(&sql_out)?;
        let mut vals: Vec<&dyn rusqlite::ToSql> = vec![&project];
        for t in DEGREE_TYPES {
            vals.push(t);
        }
        let rows = stmt.query_map(vals.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out_map = HashMap::new();
        for row in rows {
            let (id, count) = row?;
            out_map.insert(id, count);
        }
        Ok((in_map, out_map))
    }

    fn has_relationship(&self, node_id: i64, relationship: &str) -> StoreResult<bool> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM edges WHERE (source_id=?1 OR target_id=?1) AND type=?2)",
            params![node_id, relationship],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    /// BM25 full-text search over the current project.
    pub fn bm25(
        &self,
        project: &str,
        query: &str,
        file_pattern: Option<&str>,
        label: Option<&str>,
        kind: Option<&str>,
        namespace: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> StoreResult<Option<SearchOutput>> {
        let tokens = split_text(query);
        if tokens.is_empty() {
            return Ok(None);
        }
        let match_expr = tokens
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut sql = String::from(
            "SELECT n.id,n.project,n.label,n.name,n.qualified_name,n.file_path,n.start_line,n.end_line,n.properties \
             FROM nodes_fts f JOIN nodes n ON n.id=f.rowid \
             WHERE nodes_fts MATCH ?1 AND n.project=?2",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(match_expr), Box::new(project.to_string())];
        let mut next = 3;
        if let Some(label) = label {
            sql.push_str(&format!(" AND n.label=?{next}"));
            args.push(Box::new(label.to_string()));
            next += 1;
        }
        if let Some(glob) = file_pattern {
            sql.push_str(&format!(" AND n.file_path GLOB ?{next}"));
            args.push(Box::new(glob.to_string()));
            next += 1;
        }
        if let Some(kind) = kind {
            sql.push_str(&format!(
                " AND json_valid(n.properties) AND json_extract(n.properties,'$.kind')=?{next}"
            ));
            args.push(Box::new(kind.to_string()));
            next += 1;
        }
        if let Some(ns) = namespace {
            sql.push_str(&format!(
                " AND json_valid(n.properties) AND (json_extract(n.properties,'$.namespace')=?{next} \
                 OR json_extract(n.properties,'$.effective_namespace')=?{next})"
            ));
            args.push(Box::new(ns.to_string()));
            next += 1;
        }
        let _ = next;
        let count_sql = {
            let from = sql.find(" FROM nodes_fts").expect("select prefix present");
            let mut count = String::from("SELECT COUNT(*)");
            count.push_str(&sql[from..]);
            count
        };
        let total: i64 = {
            let mut stmt = self.conn.prepare(&count_sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            stmt.query_row(refs.as_slice(), |row| row.get(0))?
        };
        sql.push_str(&format!(
            " ORDER BY bm25(nodes_fts,10.0,5.0,1.0,1.0,2.0), n.id LIMIT {} OFFSET {}",
            limit.max(1),
            offset
        ));
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), row_to_node)?;
        let nodes: Vec<Node> = rows.collect::<Result<Vec<_>, _>>()?;
        let mut hits = Vec::with_capacity(nodes.len());
        for node in nodes {
            let (in_deg, out_deg) = self.node_degree(node.id, &[])?;
            hits.push(SearchHit {
                node,
                in_degree: in_deg,
                out_degree: out_deg,
                connected_names: Vec::new(),
            });
        }
        Ok(Some(SearchOutput { hits, total }))
    }

    pub fn nodes_with_label(&self, project: &str, label: &str) -> StoreResult<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND label=?2 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![project, label], row_to_node)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn nodes_by_label_like(&self, project: &str, pattern: &str) -> StoreResult<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND label LIKE ?2 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![project, pattern], row_to_node)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

fn build_regex(pattern: Option<&str>, case_sensitive: bool) -> StoreResult<Option<Regex>> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let pattern = if case_sensitive {
        pattern.to_string()
    } else {
        format!("(?i:{pattern})")
    };
    Regex::new(&pattern)
        .map(Some)
        .map_err(|e| gry_core::GryError::invalid(format!("invalid regex {pattern:?}: {e}")))
}

fn is_entry_point(node: &Node, in_deg: i64, out_deg: i64) -> bool {
    if node.property_bool("is_entry_point").unwrap_or(false) {
        return true;
    }
    if labels::is_callable(&node.label) {
        return in_deg == 0 && out_deg > 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_store;

    #[test]
    fn structural_filters() {
        let store = test_store();
        let mut p = SearchParams {
            project: "p".to_string(),
            label: Some("Function".to_string()),
            limit: 50,
            ..Default::default()
        };
        let out = store.search(&p).unwrap();
        assert!(out.total >= 3);
        p.name_pattern = Some("^parse".to_string());
        let out = store.search(&p).unwrap();
        assert_eq!(out.total, 1);
        p.name_pattern = Some("(".to_string());
        assert!(store.search(&p).is_err());
    }

    #[test]
    fn degree_and_entry_point_filters() {
        let store = test_store();
        let p = SearchParams {
            project: "p".to_string(),
            min_degree: Some(1),
            limit: 50,
            ..Default::default()
        };
        let out = store.search(&p).unwrap();
        assert!(out.total >= 1);
        let p = SearchParams {
            project: "p".to_string(),
            exclude_entry_points: true,
            label: Some("Function".to_string()),
            limit: 50,
            ..Default::default()
        };
        let out = store.search(&p).unwrap();
        assert!(out.hits.iter().all(|h| h.node.name != "entryHandler"));
    }

    #[test]
    fn bm25_finds_camel_case() {
        let store = test_store();
        let out = store
            .bm25("p", "parse request", None, None, None, None, 10, 0)
            .unwrap()
            .unwrap();
        assert!(out.hits.iter().any(|h| h.node.name == "parseRequest"));
    }
}
