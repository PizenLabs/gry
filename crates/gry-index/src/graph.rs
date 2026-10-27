//! Deterministic in-memory graph construction before atomic publication.

use gry_core::fqn::{fqn_compute, fqn_folder, fqn_module_dir, FILE_QN_NAME};
use gry_core::types::{labels, NewNode, Node};
use gry_store::EdgeSpec;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

pub struct GraphBuilder {
    pub project: String,
    pub root_path: String,
    pub mode: String,
    pub nodes: BTreeMap<String, NewNode>,
    pub edges: BTreeMap<String, EdgeSpec>,
    pub vectors: HashMap<String, Vec<i8>>,
    pub file_qn: HashMap<String, String>,
    pub module_qn: HashMap<String, String>,
    /// Nodes that already exist in the published graph but are not part of
    /// this (incremental) subset. Edges may target them without reinserting.
    pub external_qns: std::collections::HashSet<String>,
}

impl GraphBuilder {
    pub fn new(project: &str, root_path: &str, mode: &str) -> Self {
        Self {
            project: project.to_string(),
            root_path: root_path.to_string(),
            mode: mode.to_string(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            vectors: HashMap::new(),
            file_qn: HashMap::new(),
            module_qn: HashMap::new(),
            external_qns: std::collections::HashSet::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_node(
        &mut self,
        label: &str,
        name: &str,
        qn: &str,
        file_path: &str,
        start_line: i32,
        end_line: i32,
        properties: Map<String, Value>,
    ) -> String {
        if qn.is_empty() || name.is_empty() {
            return qn.to_string();
        }
        let mut props = properties;
        props
            .entry("label".to_string())
            .or_insert_with(|| Value::String(label.to_string()));
        if let Some(existing) = self.nodes.get_mut(qn) {
            if existing.label == labels::FOLDER && label == labels::MODULE {
                existing.label = labels::MODULE.to_string();
                existing.name = name.to_string();
            }
            // Merge properties; newer extraction wins for scalar fields.
            if let Value::Object(ref mut map) = existing.properties {
                for (k, v) in props {
                    if k == "variants" {
                        if let (Value::Array(new_items), Some(Value::Array(old))) =
                            (&v, map.get_mut("variants"))
                        {
                            old.extend(new_items.clone());
                        } else {
                            map.insert(k, v);
                        }
                    } else if !map.contains_key(&k) || k == "docstring" || k == "confidence" {
                        map.insert(k, v);
                    }
                }
            }
            if existing.file_path != file_path
                && !file_path.is_empty()
                && !existing.file_path.is_empty()
            {
                // A route is often exercised in a test before the production
                // registration is visited. Keep the production file as the
                // canonical location and record the test as a variant; a
                // route that points at a test file is misleading provenance.
                let promote_production_route = existing.label == labels::ROUTE
                    && is_test_path(&existing.file_path)
                    && !is_test_path(file_path);
                if promote_production_route {
                    let old = serde_json::json!({
                        "file_path": existing.file_path,
                        "start_line": existing.start_line,
                        "end_line": existing.end_line,
                    });
                    existing.file_path = file_path.to_string();
                    existing.start_line = start_line;
                    existing.end_line = end_line;
                    if let Value::Object(ref mut map) = existing.properties {
                        match map.get_mut("variants") {
                            Some(Value::Array(items)) => items.push(old),
                            _ => {
                                map.insert("variants".to_string(), Value::Array(vec![old]));
                            }
                        }
                    }
                    return qn.to_string();
                }
                let variant = serde_json::json!({
                    "file_path": file_path,
                    "start_line": start_line,
                    "end_line": end_line,
                });
                if let Value::Object(ref mut map) = existing.properties {
                    match map.get_mut("variants") {
                        Some(Value::Array(items)) => items.push(variant),
                        _ => {
                            map.insert("variants".to_string(), Value::Array(vec![variant]));
                        }
                    }
                }
            } else if start_line < existing.start_line || existing.start_line == 0 {
                existing.start_line = start_line;
                existing.end_line = end_line;
            } else if end_line > existing.end_line {
                existing.end_line = end_line;
            }
            return qn.to_string();
        }
        let node = NewNode {
            label: label.to_string(),
            name: name.to_string(),
            qualified_name: qn.to_string(),
            file_path: file_path.to_string(),
            start_line,
            end_line,
            properties: Value::Object(props),
        };
        self.nodes.insert(qn.to_string(), node);
        qn.to_string()
    }

    pub fn add_edge(
        &mut self,
        source_qn: &str,
        target_qn: &str,
        edge_type: &str,
        properties: Map<String, Value>,
    ) {
        if source_qn.is_empty() || target_qn.is_empty() || source_qn == target_qn {
            return;
        }
        if !self.has_node_or_external(source_qn) || !self.has_node_or_external(target_qn) {
            return;
        }
        let local_name = if edge_type == "IMPORTS" {
            properties
                .get("local_name")
                .and_then(Value::as_str)
                .unwrap_or("")
        } else {
            ""
        };
        let key = format!("{source_qn}\u{1f}{target_qn}\u{1f}{edge_type}\u{1f}{local_name}");
        if let Some(existing) = self.edges.get_mut(&key) {
            if let Value::Object(dst) = &mut existing.properties {
                for (k, v) in properties {
                    dst.insert(k, v);
                }
            }
            return;
        }
        self.edges.insert(
            key,
            EdgeSpec {
                source_qn: source_qn.to_string(),
                target_qn: target_qn.to_string(),
                edge_type: edge_type.to_string(),
                properties: Value::Object(properties),
            },
        );
    }

    pub fn add_edge_spec(&mut self, spec: gry_parse::InfraEdge) {
        let props = spec.properties;
        self.add_edge(&spec.source_qn, &spec.target_qn, &spec.edge_type, props);
    }

    pub fn set_node_property(&mut self, qn: &str, key: &str, value: Value) {
        if let Some(node) = self.nodes.get_mut(qn) {
            if let Value::Object(ref mut map) = node.properties {
                map.insert(key.to_string(), value);
            }
        }
    }

    pub fn add_vector(&mut self, qn: &str, vector: Vec<i8>) {
        if self.nodes.contains_key(qn) && vector.len() == gry_core::nomic::DIM {
            self.vectors.insert(qn.to_string(), vector);
        }
    }

    pub fn add_project(&mut self) {
        let mut props = Map::new();
        props.insert(
            "root_path".to_string(),
            Value::String(self.root_path.clone()),
        );
        props.insert("mode".to_string(), Value::String(self.mode.clone()));
        let project = self.project.clone();
        self.add_node(labels::PROJECT, &project, &project, "", 0, 0, props);
    }

    pub fn ensure_folder(&mut self, dir: &str) {
        if dir.is_empty() {
            return;
        }
        let qn = fqn_folder(&self.project, dir);
        if self.nodes.contains_key(&qn) {
            return;
        }
        let name = dir.rsplit('/').next().unwrap_or(dir);
        let mut props = Map::new();
        props.insert("path".to_string(), Value::String(dir.to_string()));
        self.add_node(labels::FOLDER, name, &qn, "", 0, 0, props);
        let parent = dir.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let parent_qn = if parent.is_empty() {
            self.project.clone()
        } else {
            self.ensure_folder(parent);
            fqn_folder(&self.project, parent)
        };
        self.add_edge(&parent_qn, &qn, "CONTAINS_FOLDER", Map::new());
    }

    pub fn file_node(&mut self, rel_path: &str, language: &str, size: i64) -> String {
        let qn = fqn_compute(&self.project, rel_path, Some(FILE_QN_NAME));
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        let mut props = Map::new();
        props.insert("language".to_string(), Value::String(language.to_string()));
        props.insert("size".to_string(), Value::Number(size.into()));
        self.add_node(labels::FILE, name, &qn, rel_path, 0, 0, props);
        self.file_qn.insert(rel_path.to_string(), qn.clone());
        let dir = rel_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let parent_qn = if dir.is_empty() {
            self.project.clone()
        } else {
            self.ensure_folder(dir);
            fqn_folder(&self.project, dir)
        };
        self.add_edge(&parent_qn, &qn, "CONTAINS_FILE", Map::new());
        qn
    }

    pub fn module_node(&mut self, rel_path: &str, language: &str, module_is_dir: bool) -> String {
        let qn = fqn_module_dir(&self.project, rel_path, module_is_dir);
        let name = if module_is_dir {
            let dir = rel_path
                .rsplit_once('/')
                .map(|(d, _)| d)
                .unwrap_or(&self.project);
            dir.rsplit('/').next().unwrap_or(dir).to_string()
        } else {
            rel_path
                .rsplit_once('/')
                .map(|(_, f)| f)
                .unwrap_or(rel_path)
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(rel_path)
                .to_string()
        };
        let mut props = Map::new();
        props.insert("language".to_string(), Value::String(language.to_string()));
        props.insert("module_is_dir".to_string(), Value::Bool(module_is_dir));
        self.add_node(labels::MODULE, &name, &qn, rel_path, 0, 0, props);
        self.module_qn.insert(rel_path.to_string(), qn.clone());
        let file_qn = self.file_qn.get(rel_path).cloned();
        if let Some(file_qn) = file_qn {
            self.add_edge(&file_qn, &qn, "DEFINES", Map::new());
        }
        qn
    }

    pub fn module_for(&self, rel_path: &str) -> Option<&String> {
        self.module_qn.get(rel_path)
    }

    pub fn file_for(&self, rel_path: &str) -> Option<&String> {
        self.file_qn.get(rel_path)
    }

    pub fn has_node(&self, qn: &str) -> bool {
        self.nodes.contains_key(qn)
    }

    pub fn has_node_or_external(&self, qn: &str) -> bool {
        self.nodes.contains_key(qn) || self.external_qns.contains(qn)
    }

    pub fn seed_known(
        &mut self,
        file_qn: HashMap<String, String>,
        module_qn: HashMap<String, String>,
    ) {
        for (rel, qn) in &file_qn {
            self.file_qn
                .entry(rel.clone())
                .or_insert_with(|| qn.clone());
        }
        for (rel, qn) in &module_qn {
            self.module_qn
                .entry(rel.clone())
                .or_insert_with(|| qn.clone());
        }
        self.external_qns.extend(file_qn.into_values());
        self.external_qns.extend(module_qn.into_values());
    }

    pub fn ensure_node(&mut self, label: &str, name: &str, qn: &str, props: Map<String, Value>) {
        if !self.nodes.contains_key(qn) {
            self.add_node(label, name, qn, "", 0, 0, props);
        }
    }

    pub fn finish(self) -> (Vec<NewNode>, Vec<EdgeSpec>, HashMap<String, Vec<i8>>) {
        (
            self.nodes.into_values().collect(),
            self.edges.into_values().collect(),
            self.vectors,
        )
    }
}

pub fn qn_symbol(project: &str, rel_path: &str, scope: &[String], name: &str) -> String {
    let mut symbol = String::new();
    for part in scope {
        symbol.push_str(part);
        symbol.push('.');
    }
    symbol.push_str(name);
    fqn_compute(project, rel_path, Some(&symbol))
}

pub fn qn_module(project: &str, rel_path: &str, module_is_dir: bool) -> String {
    fqn_module_dir(project, rel_path, module_is_dir)
}

pub fn qn_file(project: &str, rel_path: &str) -> String {
    fqn_compute(project, rel_path, Some(FILE_QN_NAME))
}

pub fn node_to_new(node: &Node) -> NewNode {
    NewNode {
        label: node.label.clone(),
        name: node.name.clone(),
        qualified_name: node.qualified_name.clone(),
        file_path: node.file_path.clone(),
        start_line: node.start_line,
        end_line: node.end_line,
        properties: node.properties.clone(),
    }
}

pub fn props(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
        .collect()
}

fn is_test_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_route_replaces_a_test_route_as_canonical() {
        let mut builder = GraphBuilder::new("p", "/tmp/p", "full");
        builder.add_node(
            labels::ROUTE,
            "/profile",
            "__route__GET__/profile",
            "api/controller/profile_controller_test.go",
            50,
            50,
            props(&[("method", "GET"), ("url_path", "/profile")]),
        );
        builder.add_node(
            labels::ROUTE,
            "/profile",
            "__route__GET__/profile",
            "api/route/profile_route.go",
            20,
            20,
            props(&[("method", "GET"), ("url_path", "/profile")]),
        );
        let node = builder.nodes.get("__route__GET__/profile").unwrap();
        assert_eq!(node.file_path, "api/route/profile_route.go");
        assert_eq!(node.start_line, 20);
        let variants = node.properties["variants"].as_array().unwrap();
        assert!(variants
            .iter()
            .any(|v| { v["file_path"] == "api/controller/profile_controller_test.go" }));
    }
}
