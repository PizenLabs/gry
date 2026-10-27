//! Serializable per-file extraction result.
//!
//! The model is project-independent so it can be cached and reused by an
//! incremental index, including under a project-name change. Qualified names are
//! computed by the indexing pipeline from `scope` + `name` + `rel_path`.

use gry_core::profile::AstProfile;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileExtract {
    pub language: String,
    pub sha256: String,
    pub size: u64,
    pub lines: u32,
    #[serde(default)]
    pub defs: Vec<Def>,
    #[serde(default)]
    pub calls: Vec<CallSite>,
    #[serde(default)]
    pub imports: Vec<Import>,
    #[serde(default)]
    pub usages: Vec<Usage>,
    #[serde(default)]
    pub routes: Vec<RouteDef>,
    #[serde(default)]
    pub env_accesses: Vec<EnvAccess>,
    #[serde(default)]
    pub channels: Vec<ChannelUse>,
    #[serde(default)]
    pub spawns: Vec<SpawnCall>,
    #[serde(default)]
    pub http_calls: Vec<HttpCall>,
    #[serde(default)]
    pub file_refs: Vec<FileRef>,
    #[serde(default)]
    pub config_keys: Vec<ConfigKey>,
    #[serde(default)]
    pub package: Option<PackageInfo>,
    #[serde(default)]
    pub sections: Vec<DocSection>,
    #[serde(default)]
    pub doc_refs: Vec<DocRef>,
    #[serde(default)]
    pub adr: Option<AdrRecord>,
    #[serde(default)]
    pub diagrams: Vec<Diagram>,
    #[serde(default)]
    pub openapi: Vec<OpenApiOperation>,
    #[serde(default)]
    pub infra_nodes: Vec<InfraNode>,
    #[serde(default)]
    pub infra_edges: Vec<InfraEdge>,
    #[serde(default)]
    pub parse_gaps: Vec<ParseGap>,
    #[serde(default)]
    pub file_docstring: Option<String>,
    #[serde(default)]
    pub minhash: Option<String>,
    #[serde(default)]
    pub profile: Option<AstProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Def {
    pub label: String,
    pub name: String,
    /// Enclosing containers, outermost first. Empty for module-level symbols.
    #[serde(default)]
    pub scope: Vec<String>,
    pub start_line: i32,
    pub end_line: i32,
    #[serde(default)]
    pub docstring: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default)]
    pub return_type: Option<String>,
    /// `(name, is_implements)` pairs.
    #[serde(default)]
    pub bases: Vec<(String, bool)>,
    #[serde(default)]
    pub decorators: Vec<String>,
    #[serde(default)]
    pub complexity: Option<u32>,
    #[serde(default)]
    pub cognitive: Option<u32>,
    #[serde(default)]
    pub loop_depth: Option<u32>,
    #[serde(default)]
    pub is_test: bool,
    #[serde(default)]
    pub is_entry_point: bool,
    #[serde(default)]
    pub exported: bool,
    #[serde(default)]
    pub properties: Map<String, Value>,
}

impl Def {
    pub fn scoped_name(&self) -> String {
        if self.scope.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.scope.join("."), self.name)
        }
    }

    pub fn container_scope(&self) -> Vec<String> {
        let mut scope = self.scope.clone();
        scope.push(self.name.clone());
        scope
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallSite {
    pub name: String,
    #[serde(default)]
    pub receiver: Option<String>,
    #[serde(default)]
    pub receiver_type: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub scope: Vec<String>,
    pub line: i32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub is_reference: bool,
    #[serde(default)]
    pub properties: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Import {
    pub specifier: String,
    #[serde(default)]
    pub local_name: Option<String>,
    #[serde(default)]
    pub imported_name: Option<String>,
    pub line: i32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub is_relative: bool,
    #[serde(default)]
    pub properties: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub name: String,
    #[serde(default)]
    pub scope: Vec<String>,
    pub line: i32,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouteDef {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub handler: Option<String>,
    /// Receiver type of the handler expression when the extractor could infer
    /// it (for example `tc.Create` where `tc` is a `*TaskController`). This lets
    /// the indexer link the route to the handler across files instead of
    /// falling back to the router function.
    #[serde(default)]
    pub handler_type: Option<String>,
    pub line: i32,
    #[serde(default)]
    pub framework: String,
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvAccess {
    pub key: String,
    pub line: i32,
    #[serde(default)]
    pub scope: Vec<String>,
    /// `read` or `write`.
    #[serde(default)]
    pub mode: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelUse {
    pub name: String,
    pub line: i32,
    #[serde(default)]
    pub scope: Vec<String>,
    /// `emit` or `listen`.
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub broker: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpawnCall {
    pub program: String,
    pub line: i32,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub dynamic: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HttpCall {
    pub method: String,
    pub url_path: String,
    #[serde(default)]
    pub host: Option<String>,
    pub line: i32,
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileRef {
    pub path: String,
    pub line: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigKey {
    pub key: String,
    pub line: i32,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub format: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    #[serde(default)]
    pub ecosystem: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocSection {
    pub level: u32,
    pub title: String,
    pub start_line: i32,
    pub end_line: i32,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocRef {
    pub raw: String,
    pub line: i32,
    #[serde(default)]
    pub syntax: String,
    /// `code`, `file`, `adr`, or `link`.
    #[serde(default)]
    pub target_kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdrRecord {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub deciders: Vec<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    pub docstring: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagram {
    pub name: String,
    pub line: i32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub elements: Vec<DiagramElement>,
    #[serde(default)]
    pub arrows: Vec<DiagramArrow>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagramElement {
    pub id: String,
    pub label: String,
    pub line: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagramArrow {
    pub source: String,
    pub target: String,
    pub line: i32,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenApiOperation {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    pub line: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InfraNode {
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    #[serde(default)]
    pub properties: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InfraEdge {
    pub source_qn: String,
    pub target_qn: String,
    pub edge_type: String,
    #[serde(default)]
    pub properties: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParseGap {
    pub start_line: i32,
    pub end_line: i32,
    #[serde(default)]
    pub reason: String,
}
