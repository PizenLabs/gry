//! Bounded graph traversal (callers, callees, data flow, infrastructure paths).

use crate::error::StoreResult;
use crate::Store;
use gry_core::types::Node;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
    Both,
}

impl Direction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inbound" => Some(Direction::Inbound),
            "outbound" => Some(Direction::Outbound),
            "both" | "" => Some(Direction::Both),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Direction::Inbound => "inbound",
            Direction::Outbound => "outbound",
            Direction::Both => "both",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TraceParams {
    pub project: String,
    pub start_qn: String,
    pub direction: Direction,
    pub depth: usize,
    pub edge_types: Vec<String>,
    pub include_tests: bool,
    pub limit: usize,
    pub parameter_name: Option<String>,
}

impl TraceParams {
    pub fn new(project: impl Into<String>, start_qn: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            start_qn: start_qn.into(),
            direction: Direction::Both,
            depth: 3,
            edge_types: Vec::new(),
            include_tests: false,
            limit: 100,
            parameter_name: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TraceHop {
    pub node: Node,
    pub hop: usize,
    pub direction: String,
    pub via_edge: String,
    pub via_qn: String,
    pub via_direction: String,
    pub edge_properties: Value,
    pub observed: bool,
    pub observed_count: i64,
}

#[derive(Debug, Clone)]
pub struct TraceOutput {
    pub start: Node,
    pub hops: Vec<TraceHop>,
    pub truncated: bool,
    pub total: i64,
    /// Nodes already visited when the limit truncated the traversal.
    pub visited: usize,
}

impl Store {
    pub fn trace(&self, params: &TraceParams) -> StoreResult<Option<TraceOutput>> {
        let Some(start) = self.resolve_node(&params.project, &params.start_qn)? else {
            return Ok(None);
        };
        let mut node_cache: HashMap<i64, Node> = HashMap::new();
        node_cache.insert(start.id, start.clone());
        let observed = self.observed_call_lookup(&params.project, None)?;

        let depth = params.depth.clamp(1, 15);
        let limit = params.limit.clamp(1, 5000);
        let mut visited: HashSet<i64> = HashSet::new();
        visited.insert(start.id);
        let mut queue: VecDeque<(i64, usize)> = VecDeque::new();
        queue.push_back((start.id, 0));
        let mut hops = Vec::new();
        let mut truncated = false;

        while let Some((node_id, hop)) = queue.pop_front() {
            if hop >= depth {
                continue;
            }
            let edges =
                self.edges_for_nodes(&[node_id], &params.edge_types, params.direction.as_str())?;
            for edge in edges {
                let (neighbor_id, traversal_direction) = if edge.source_id == node_id {
                    (edge.target_id, "outbound")
                } else {
                    (edge.source_id, "inbound")
                };
                if params.direction == Direction::Inbound && traversal_direction != "inbound" {
                    continue;
                }
                if params.direction == Direction::Outbound && traversal_direction != "outbound" {
                    continue;
                }
                if !visited.insert(neighbor_id) {
                    continue;
                }
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    node_cache.entry(neighbor_id)
                {
                    if let Some(node) = self.node_by_id(&params.project, neighbor_id)? {
                        entry.insert(node);
                    }
                }
                let Some(neighbor) = node_cache.get(&neighbor_id) else {
                    continue;
                };
                if !params.include_tests && is_test_node(neighbor) {
                    continue;
                }
                if let Some(param) = &params.parameter_name {
                    let matches = edge
                        .properties
                        .get("params")
                        .map(|v| v.to_string().contains(param))
                        .unwrap_or(false)
                        || edge
                            .properties
                            .get("parameter")
                            .and_then(Value::as_str)
                            .map(|p| p == param)
                            .unwrap_or(false);
                    if !matches {
                        continue;
                    }
                }
                if hops.len() >= limit {
                    truncated = true;
                    break;
                }
                let via = node_cache.get(&node_id);
                let (obs, count) = observed
                    .get(&(
                        via.map(|n| n.qualified_name.clone()).unwrap_or_default(),
                        neighbor.qualified_name.clone(),
                    ))
                    .copied()
                    .unwrap_or((false, 0));
                hops.push(TraceHop {
                    node: neighbor.clone(),
                    hop: hop + 1,
                    direction: params.direction.as_str().to_string(),
                    via_edge: edge.edge_type.clone(),
                    via_qn: via.map(|n| n.qualified_name.clone()).unwrap_or_default(),
                    via_direction: traversal_direction.to_string(),
                    edge_properties: edge.properties.clone(),
                    observed: obs,
                    observed_count: count,
                });
                queue.push_back((neighbor_id, hop + 1));
            }
            if truncated {
                break;
            }
        }

        // Stable, nearest-first ordering: hop, then direction, then QN.
        hops.sort_by(|a, b| {
            a.hop
                .cmp(&b.hop)
                .then(a.direction.cmp(&b.direction))
                .then(a.node.qualified_name.cmp(&b.node.qualified_name))
        });
        let total = hops.len() as i64;
        Ok(Some(TraceOutput {
            start,
            hops,
            truncated,
            total,
            visited: visited.len(),
        }))
    }

    pub fn observed_call_lookup(
        &self,
        project: &str,
        label: Option<&str>,
    ) -> StoreResult<HashMap<(String, String), (bool, i64)>> {
        let rows = self.observed_calls(project, label)?;
        let mut map = HashMap::new();
        for (caller, callee, _label, count) in rows {
            map.entry((caller, callee))
                .and_modify(|(_, c): &mut (bool, i64)| *c += count)
                .or_insert((true, count));
        }
        Ok(map)
    }

    /// Return direct neighbors of a node, used by architecture and impact.
    pub fn neighbors_of(
        &self,
        project: &str,
        node_id: i64,
        edge_types: &[String],
    ) -> StoreResult<Vec<(Node, String, bool)>> {
        let edges = self.edges_for_nodes(&[node_id], edge_types, "both")?;
        let mut out = Vec::new();
        for edge in edges {
            let (other, outgoing) = if edge.source_id == node_id {
                (edge.target_id, true)
            } else {
                (edge.source_id, false)
            };
            if let Some(node) = self.node_by_id(project, other)? {
                out.push((node, edge.edge_type, outgoing));
            }
        }
        Ok(out)
    }

    pub fn node_by_id(&self, project: &str, id: i64) -> StoreResult<Option<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,properties
             FROM nodes WHERE project=?1 AND id=?2",
        )?;
        Ok(stmt
            .query_row(rusqlite::params![project, id], crate::row_to_node)
            .ok())
    }
}

pub fn is_test_node(node: &Node) -> bool {
    if node.property_bool("is_test").unwrap_or(false) {
        return true;
    }
    let path = node.file_path.to_ascii_lowercase();
    let name = node.name.to_ascii_lowercase();
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/__tests__/")
        || path.ends_with("_test.go")
        || path.ends_with("_test.py")
        || path.ends_with("_test.rs")
        || path.ends_with(".test.ts")
        || path.ends_with(".test.js")
        || path.ends_with(".spec.ts")
        || path.ends_with(".spec.js")
        || name.starts_with("test_")
        || name.starts_with("test") && path.contains("test")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_store;

    #[test]
    fn traverses_callers_and_callees() {
        let store = test_store();
        let mut p = TraceParams::new("p", "p.src.main.main");
        p.direction = Direction::Outbound;
        p.depth = 3;
        p.edge_types = vec!["CALLS".to_string()];
        let out = store.trace(&p).unwrap().unwrap();
        assert!(out.hops.iter().any(|h| h.node.name == "parseRequest"));
        assert!(out.total > 0);

        p.start_qn = "p.src.lib.parseRequest".to_string();
        p.direction = Direction::Inbound;
        let out = store.trace(&p).unwrap().unwrap();
        assert!(out.hops.iter().any(|h| h.node.name == "main"));
    }

    #[test]
    fn depth_and_limits_bound_work() {
        let store = test_store();
        let mut p = TraceParams::new("p", "p.src.main.main");
        p.depth = 1;
        p.limit = 1;
        p.edge_types = vec!["CALLS".to_string()];
        let out = store.trace(&p).unwrap().unwrap();
        assert!(out.hops.len() <= 1);
    }

    #[test]
    fn missing_start_is_none() {
        let store = test_store();
        assert!(store
            .trace(&TraceParams::new("p", "p.nope"))
            .unwrap()
            .is_none());
    }
}
