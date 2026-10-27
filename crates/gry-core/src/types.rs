//! gry domain model: nodes, edges, identifiers and relationship vocabulary.
//!
//! The model defines gry's graph contract: node identity is
//! `(project, qualified_name)`, edge identity is
//! `(source, target, type, local_name)` and properties are free-form JSON.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Canonical node labels. Kept as string constants because the public graph
/// vocabulary is string-based (MCP filters, Cypher labels, user-visible output).
pub mod labels {
    pub const PROJECT: &str = "Project";
    pub const PACKAGE: &str = "Package";
    pub const FOLDER: &str = "Folder";
    pub const FILE: &str = "File";
    pub const MODULE: &str = "Module";
    pub const SECTION: &str = "Section";
    pub const CLASS: &str = "Class";
    pub const STRUCT: &str = "Struct";
    pub const INTERFACE: &str = "Interface";
    pub const TRAIT: &str = "Trait";
    pub const ENUM: &str = "Enum";
    pub const TYPE: &str = "Type";
    pub const FUNCTION: &str = "Function";
    pub const METHOD: &str = "Method";
    pub const FIELD: &str = "Field";
    pub const VARIABLE: &str = "Variable";
    pub const CONSTANT: &str = "Constant";
    pub const MACRO: &str = "Macro";
    pub const DECORATOR: &str = "Decorator";
    pub const ROUTE: &str = "Route";
    pub const CHANNEL: &str = "Channel";
    pub const TABLE: &str = "Table";
    pub const PROCESS: &str = "Process";
    pub const ENV_VAR: &str = "EnvVar";
    pub const RESOURCE: &str = "Resource";
    pub const CHART: &str = "Chart";
    pub const IMAGE: &str = "Image";
    pub const BRANCH: &str = "Branch";
    pub const ADR: &str = "ADR";
    pub const DIAGRAM: &str = "Diagram";
    pub const DIAGRAM_ELEMENT: &str = "DiagramElement";

    pub const ALL: &[&str] = &[
        PROJECT,
        PACKAGE,
        FOLDER,
        FILE,
        MODULE,
        SECTION,
        CLASS,
        STRUCT,
        INTERFACE,
        TRAIT,
        ENUM,
        TYPE,
        FUNCTION,
        METHOD,
        FIELD,
        VARIABLE,
        CONSTANT,
        MACRO,
        DECORATOR,
        ROUTE,
        CHANNEL,
        TABLE,
        PROCESS,
        ENV_VAR,
        RESOURCE,
        CHART,
        IMAGE,
        BRANCH,
        ADR,
        DIAGRAM,
        DIAGRAM_ELEMENT,
    ];

    /// Labels that denote a declaration (excluded from file outlines).
    pub fn is_container(label: &str) -> bool {
        matches!(
            label,
            CLASS | STRUCT | INTERFACE | TRAIT | ENUM | TYPE | MODULE | PACKAGE
        )
    }

    pub fn is_callable(label: &str) -> bool {
        matches!(label, FUNCTION | METHOD)
    }

    pub fn is_type_like(label: &str) -> bool {
        matches!(label, CLASS | STRUCT | INTERFACE | TRAIT | ENUM | TYPE)
    }

    pub fn is_infrastructure(label: &str) -> bool {
        matches!(
            label,
            ROUTE | CHANNEL | PROCESS | ENV_VAR | RESOURCE | CHART | IMAGE
        )
    }
}

/// Canonical edge types.
pub mod edges {
    pub const CONTAINS_FOLDER: &str = "CONTAINS_FOLDER";
    pub const CONTAINS_FILE: &str = "CONTAINS_FILE";
    pub const DEFINES: &str = "DEFINES";
    pub const DEFINES_METHOD: &str = "DEFINES_METHOD";
    pub const IMPORTS: &str = "IMPORTS";
    pub const CALLS: &str = "CALLS";
    pub const CALL_REFERENCE: &str = "CALL_REFERENCE";
    pub const USAGE: &str = "USAGE";
    pub const INHERITS: &str = "INHERITS";
    pub const IMPLEMENTS: &str = "IMPLEMENTS";
    pub const OVERRIDE: &str = "OVERRIDE";
    pub const DECORATES: &str = "DECORATES";
    pub const RAISES: &str = "RAISES";
    pub const THROWS: &str = "THROWS";
    pub const WRITES: &str = "WRITES";
    pub const DATA_FLOWS: &str = "DATA_FLOWS";
    pub const HTTP_CALLS: &str = "HTTP_CALLS";
    pub const GRPC_CALLS: &str = "GRPC_CALLS";
    pub const ASYNC_CALLS: &str = "ASYNC_CALLS";
    pub const EMITS: &str = "EMITS";
    pub const LISTENS_ON: &str = "LISTENS_ON";
    pub const HANDLES: &str = "HANDLES";
    pub const SPAWNS: &str = "SPAWNS";
    pub const CONFIGURES: &str = "CONFIGURES";
    pub const INFRA_MAPS: &str = "INFRA_MAPS";
    pub const CONNECTS_TO: &str = "CONNECTS_TO";
    pub const ROUTES_TO: &str = "ROUTES_TO";
    pub const USES_CONFIG: &str = "USES_CONFIG";
    pub const USES_SECRET: &str = "USES_SECRET";
    pub const USES_NETWORK: &str = "USES_NETWORK";
    pub const USES_VOLUME: &str = "USES_VOLUME";
    pub const DEPLOYS: &str = "DEPLOYS";
    pub const DEPENDS_ON: &str = "DEPENDS_ON";
    pub const REFERENCES_FILE: &str = "REFERENCES_FILE";
    pub const MENTIONS: &str = "MENTIONS";
    pub const TESTS: &str = "TESTS";
    pub const TESTS_FILE: &str = "TESTS_FILE";
    pub const SIMILAR_TO: &str = "SIMILAR_TO";
    pub const SEMANTICALLY_RELATED: &str = "SEMANTICALLY_RELATED";
    pub const FILE_CHANGES_WITH: &str = "FILE_CHANGES_WITH";
    pub const HAS_BRANCH: &str = "HAS_BRANCH";
    pub const SUPERSEDES: &str = "SUPERSEDES";
    pub const DIAGRAM_ARROW: &str = "DIAGRAM_ARROW";

    pub const CROSS_HTTP_CALLS: &str = "CROSS_HTTP_CALLS";
    pub const CROSS_GRPC_CALLS: &str = "CROSS_GRPC_CALLS";
    pub const CROSS_GRAPHQL_CALLS: &str = "CROSS_GRAPHQL_CALLS";
    pub const CROSS_TRPC_CALLS: &str = "CROSS_TRPC_CALLS";
    pub const CROSS_ASYNC_CALLS: &str = "CROSS_ASYNC_CALLS";
    pub const CROSS_CHANNEL: &str = "CROSS_CHANNEL";

    pub const ALL: &[&str] = &[
        CONTAINS_FOLDER,
        CONTAINS_FILE,
        DEFINES,
        DEFINES_METHOD,
        IMPORTS,
        CALLS,
        CALL_REFERENCE,
        USAGE,
        INHERITS,
        IMPLEMENTS,
        OVERRIDE,
        DECORATES,
        RAISES,
        THROWS,
        WRITES,
        DATA_FLOWS,
        HTTP_CALLS,
        GRPC_CALLS,
        ASYNC_CALLS,
        EMITS,
        LISTENS_ON,
        HANDLES,
        SPAWNS,
        CONFIGURES,
        INFRA_MAPS,
        CONNECTS_TO,
        ROUTES_TO,
        USES_CONFIG,
        USES_SECRET,
        USES_NETWORK,
        USES_VOLUME,
        DEPLOYS,
        DEPENDS_ON,
        REFERENCES_FILE,
        MENTIONS,
        TESTS,
        TESTS_FILE,
        SIMILAR_TO,
        SEMANTICALLY_RELATED,
        FILE_CHANGES_WITH,
        HAS_BRANCH,
        SUPERSEDES,
        DIAGRAM_ARROW,
        CROSS_HTTP_CALLS,
        CROSS_GRPC_CALLS,
        CROSS_GRAPHQL_CALLS,
        CROSS_TRPC_CALLS,
        CROSS_ASYNC_CALLS,
        CROSS_CHANNEL,
    ];

    /// Edge types counted by `search_graph` in/out degree.
    pub const SEARCH_DEGREE_TYPES: &[&str] = &[
        CALLS,
        USAGE,
        CALL_REFERENCE,
        INHERITS,
        IMPLEMENTS,
        GRPC_CALLS,
        HTTP_CALLS,
        ASYNC_CALLS,
    ];

    pub fn is_valid(edge_type: &str) -> bool {
        ALL.contains(&edge_type)
    }
}

/// One graph node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: i64,
    pub project: String,
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    #[serde(default)]
    pub properties: Value,
}

impl Node {
    pub fn property(&self, key: &str) -> Option<&Value> {
        self.properties.get(key)
    }

    pub fn property_str(&self, key: &str) -> Option<&str> {
        self.properties.get(key).and_then(Value::as_str)
    }

    pub fn property_bool(&self, key: &str) -> Option<bool> {
        match self.properties.get(key) {
            Some(Value::Bool(value)) => Some(*value),
            Some(Value::String(value)) => match value.as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            },
            Some(Value::Number(value)) => value.as_i64().map(|n| n != 0),
            _ => None,
        }
    }

    pub fn property_i64(&self, key: &str) -> Option<i64> {
        self.properties.get(key).and_then(Value::as_i64)
    }

    pub fn properties_object(&self) -> &Map<String, Value> {
        match &self.properties {
            Value::Object(map) => map,
            _ => empty_object(),
        }
    }
}

fn empty_object() -> &'static Map<String, Value> {
    static EMPTY: once_cell::sync::Lazy<Map<String, Value>> = once_cell::sync::Lazy::new(Map::new);
    &EMPTY
}

/// A node before storage assigns an id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewNode {
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub properties: Value,
}

impl NewNode {
    pub fn new(
        label: impl Into<String>,
        name: impl Into<String>,
        qualified_name: impl Into<String>,
        file_path: impl Into<String>,
        start_line: i32,
        end_line: i32,
    ) -> Self {
        Self {
            label: label.into(),
            name: name.into(),
            qualified_name: qualified_name.into(),
            file_path: file_path.into(),
            start_line,
            end_line,
            properties: Value::Object(Map::new()),
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        if let Value::Object(ref mut map) = self.properties {
            map.insert(key.into(), value.into());
        }
        self
    }
}

/// One graph edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub id: i64,
    pub project: String,
    pub source_id: i64,
    pub target_id: i64,
    pub edge_type: String,
    #[serde(default)]
    pub properties: Value,
}

impl Edge {
    pub fn local_name(&self) -> Option<&str> {
        self.properties.get("local_name").and_then(Value::as_str)
    }
}

/// An edge before storage assigns ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub edge_type: String,
    pub properties: Value,
}

impl NewEdge {
    pub fn new(source_id: i64, target_id: i64, edge_type: impl Into<String>) -> Self {
        Self {
            source_id,
            target_id,
            edge_type: edge_type.into(),
            properties: Value::Object(Map::new()),
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        if let Value::Object(ref mut map) = self.properties {
            map.insert(key.into(), value.into());
        }
        self
    }
}

/// Project metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub indexed_at: String,
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHash {
    pub rel_path: String,
    pub sha256: String,
    pub mtime_ns: i64,
    pub size: i64,
}

/// Result status for resolution/provenance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Exact,
    Import,
    SameModule,
    UniqueName,
    Suffix,
    Inferred,
    Ambiguous,
    Unresolved,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Exact => "exact",
            Resolution::Import => "import",
            Resolution::SameModule => "same_module",
            Resolution::UniqueName => "unique_name",
            Resolution::Suffix => "suffix",
            Resolution::Inferred => "inferred",
            Resolution::Ambiguous => "ambiguous",
            Resolution::Unresolved => "unresolved",
        }
    }
}

pub fn confidence_band(score: f64) -> &'static str {
    if score >= 0.70 {
        "high"
    } else if score >= 0.45 {
        "medium"
    } else if score >= 0.25 {
        "speculative"
    } else {
        ""
    }
}

/// Decode a property value as a string representation for compact output.
pub fn value_to_display(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => other.to_string(),
    }
}
