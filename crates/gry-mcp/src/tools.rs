//! MCP tool definitions, schemas and annotations.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProfile {
    All,
    Analysis,
    Scout,
}

impl ToolProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" | "" => Some(ToolProfile::All),
            "analysis" => Some(ToolProfile::Analysis),
            "scout" => Some(ToolProfile::Scout),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ToolProfile::All => "all",
            ToolProfile::Analysis => "analysis",
            ToolProfile::Scout => "scout",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
    pub always_load: bool,
}

pub fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "index_repository",
            description: "Index a repository. full/moderate add semantics; fast omits semantic edges; cross-repo-intelligence links services. Reports coverage gaps. Waits for the whole index by default; async:true starts a job and status:true polls it.",
            schema: json!({
                "type":"object",
                "properties":{
                    "repo_path":{"type":"string","description":"Repository path"},
                    "mode":{"type":"string","enum":["full","moderate","fast","cross-repo-intelligence"],"default":"full"},
                    "target_projects":{"type":"array","items":{"type":"string"},"description":"Cross-repo targets; [\"*\"] means all."},
                    "name":{"type":"string","description":"Project name override."},
                    "persistence":{"type":"boolean","default":false,"description":"Write .gry/graph.db.zst."},
                    "doc_candidates":{"type":"boolean","description":"Score documentation candidates."},
                    "async":{"type":"boolean","default":false,"description":"Start in the background and return immediately."},
                    "status":{"type":"boolean","default":false,"description":"Report the running or last index job."}
                },
                "required":["repo_path"]
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "search_graph",
            description: "Find symbols by name or text. Use name_pattern for an exact/known identifier; query is broad BM25. Rows keep qn/file/lines/degree, with production code ranked before mocks and tests. Prefer get_task_context for multi-part structural tasks.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "query":{"type":"string"},
                    "label":{"type":"string"},
                    "name_pattern":{"type":"string"},
                    "qn_pattern":{"type":"string"},
                    "file_pattern":{"type":"string"},
                    "kind":{"type":"string"},
                    "namespace":{"type":"string"},
                    "relationship":{"type":"string"},
                    "min_degree":{"type":"integer"},
                    "max_degree":{"type":"integer"},
                    "exclude_entry_points":{"type":"boolean"},
                    "include_connected":{"type":"boolean"},
                    "semantic_query":{"type":"array","items":{"type":"string"},"description":"Not with query."},
                    "semantic_limit":{"type":"integer","default":50,"minimum":0,"maximum":500},
                    "semantic_offset":{"type":"integer","default":0,"minimum":0,"maximum":99998},
                    "limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "offset":{"type":"integer","default":0,"minimum":0},
                    "max_output_tokens":{"type":"integer","default":3200,"minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"},
                    "fields":{"type":"array","items":{"type":"string"}},
                    "detail":{"type":"string","enum":["ids","default"],"default":"default"}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: true,
        },
        ToolDef {
            name: "query_graph",
            description: "Read-only Cypher for multi-hop or aggregate questions that search/trace cannot express. Default 200 rows with exact totals and continuation.",
            schema: json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string"},
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "graph":{"type":"string","enum":["code","missed"],"default":"code"},
                    "max_rows":{"type":"integer","minimum":0,"default":200},
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "cursor":{"type":"string"},
                    "max_output_tokens":{"type":"integer","minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["query"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "trace_path",
            description: "Trace callers/callees or infrastructure paths from one symbol. Accepts a QN, unique suffix, or Type.Method; an ambiguous short name returns candidates instead of guessing. Returns relationships and ranges, not source.",
            schema: json!({
                "type":"object",
                "properties":{
                    "function_name":{"type":"string"},
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "direction":{"type":"string","enum":["inbound","outbound","both"],"default":"both"},
                    "depth":{"type":"integer","default":3,"minimum":1,"maximum":15},
                    "limit":{"type":"integer","default":100,"minimum":1,"maximum":5000},
                    "max_output_tokens":{"type":"integer","default":3200,"minimum":128,"maximum":1000000},
                    "cursor":{"type":"string"},
                    "mode":{"type":"string","enum":["calls","data_flow","cross_service","topology","deploy","run"],"default":"calls"},
                    "parameter_name":{"type":"string"},
                    "edge_types":{"type":"array","items":{"type":"string"}},
                    "risk_labels":{"type":"boolean","default":false},
                    "include_tests":{"type":"boolean","default":false},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"},
                    "include_evidence":{"type":"boolean","default":false}
                },
                "required":["function_name"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: true,
        },
        ToolDef {
            name: "get_code_snippet",
            description: "Read one symbol's exact source by qualified name. Use after search_graph/get_task_context for material claims; do not re-run discovery. auto bounds source, full restores it, outline lists members.",
            schema: json!({
                "type":"object",
                "properties":{
                    "qualified_name":{"type":"string"},
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "include_neighbors":{"type":"boolean","default":false},
                    "source_mode":{"type":"string","enum":["auto","full","outline"],"default":"auto"},
                    "member_limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "member_offset":{"type":"integer","default":0,"minimum":0},
                    "start_line":{"type":"integer","minimum":1},
                    "max_lines":{"type":"integer","minimum":1,"maximum":500},
                    "max_output_tokens":{"type":"integer","minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["qualified_name"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: true,
        },
        ToolDef {
            name: "get_file_outline",
            description: "Declaration outline of one repository-relative file in source order, with optional exact label filter and paging.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "file_path":{"type":"string"},
                    "labels":{"type":"array","items":{"type":"string"},"maxItems":16},
                    "limit":{"type":"integer","minimum":1,"maximum":200,"default":100},
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "additionalProperties":false,
                "required":["file_path"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "get_graph_schema",
            description: "Node/edge counts and queryable properties. Use only to author query_graph, not as a discovery or setup step.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"},
                    "diagnostics":{"type":"string","enum":["none","full"],"default":"none"},
                    "limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "offset":{"type":"integer","default":0,"minimum":0}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "compare_graphs",
            description: "Compare two indexed snapshots by stable node/edge identities with exact totals and truncation.",
            schema: json!({
                "type":"object",
                "properties":{
                    "base_project":{"type":"string","minLength":1},
                    "target_project":{"type":"string","minLength":1},
                    "limit":{"type":"integer","minimum":1,"maximum":1000,"default":200},
                    "scan_limit":{"type":"integer","minimum":1,"maximum":10000000,"default":2000000}
                },
                "required":["base_project","target_project"],
                "additionalProperties":false
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "get_architecture",
            description: "Bounded architecture summary: structure, dependencies, routes, packages, entry points, hotspots, layers, cycles. Use when the question is about the shape of the system, not one symbol.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "path":{"type":"string","description":"Directory prefix."},
                    "aspects":{"type":"array","items":{"type":"string","enum":["all","overview","structure","dependencies","routes","languages","packages","entry_points","hotspots","boundaries","layers","file_tree","clusters","cycles","system_structure"]}},
                    "expected_generation":{"type":"string","minLength":1},
                    "max_output_tokens":{"type":"integer","default":8000,"minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "search_code",
            description: "Graph-ranked literal or regex text search over source and non-code files. Use for exact strings, config values and comments; use search_graph for symbol discovery.",
            schema: json!({
                "type":"object",
                "properties":{
                    "pattern":{"type":"string"},
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "file_pattern":{"type":"string"},
                    "path_filter":{"type":"string"},
                    "mode":{"type":"string","enum":["compact","full","files"],"default":"compact"},
                    "context":{"type":"integer","default":0,"minimum":0,"maximum":200},
                    "regex":{"type":"boolean","default":false},
                    "debug":{"type":"boolean","default":false},
                    "limit":{"type":"integer","default":10,"minimum":1,"maximum":500},
                    "result_limit":{"type":"integer","default":10,"minimum":1,"maximum":500},
                    "result_offset":{"type":"integer","default":0,"minimum":0},
                    "raw_limit":{"type":"integer","default":5,"minimum":0,"maximum":100},
                    "raw_offset":{"type":"integer","default":0,"minimum":0},
                    "directory_limit":{"type":"integer","default":20,"minimum":0,"maximum":64},
                    "directory_offset":{"type":"integer","default":0,"minimum":0},
                    "match_limit":{"type":"integer","default":8,"minimum":1,"maximum":500},
                    "source_max_lines":{"type":"integer","default":20,"minimum":1,"maximum":200},
                    "cursor":{"type":"string","description":"Opaque continuation from next_cursor."},
                    "max_output_tokens":{"type":"integer","minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["pattern"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "list_projects",
            description: "List indexed projects. Usually unnecessary: project arguments default to the indexed workspace root.",
            schema: json!({
                "type":"object",
                "properties":{
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"},
                    "detail":{"type":"string","enum":["identity","stats"],"default":"identity"},
                    "include_details":{"type":"boolean","default":false},
                    "limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "offset":{"type":"integer","default":0,"minimum":0},
                    "metadata_only":{"type":"boolean","default":false}
                }
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "delete_project",
            description: "Delete a project and all its graph data.",
            schema: json!({
                "type":"object",
                "properties":{"project":{"type":"string","description":"Optional; defaults to the indexed workspace root."}},
                "required":["project"]
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "index_status",
            description: "Readiness, counts, generation, coverage gaps and freshness. Not a setup step; get_task_context already reports freshness.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "verbose":{"type":"boolean","default":false},
                    "diagnostics":{"type":"string","enum":["none","summary","full"],"default":"none"},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "check_index_coverage",
            description: "Check whether cited paths/scopes were fully indexed. Use only when coverage or freshness is in doubt; a clean result is not proof of repository-wide completeness.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "paths":{"type":"array","items":{"type":"string"},"maxItems":128},
                    "path_limit":{"type":"integer","default":20,"minimum":1,"maximum":128},
                    "path_offset":{"type":"integer","default":0,"minimum":0},
                    "scopes":{"type":"array","items":{"type":"string"},"maxItems":32},
                    "scope_limit":{"type":"integer","default":20,"minimum":1,"maximum":1000},
                    "scope_offset":{"type":"integer","default":0,"minimum":0},
                    "diagnostics":{"type":"string","enum":["none","full"],"default":"none"},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "get_task_context",
            description: "Start here for a multi-part structural task. One bounded call resolves targets from a task or explicit targets and returns files, symbols, relationships, tests and provenance. Use depth 3 for route-to-repository chains; include tests for test-gap questions. Not for exact literal search.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "task":{"type":"string","description":"Task description used only when targets are omitted."},
                    "targets":{"type":"array","items":{"type":"string"},"maxItems":32,"description":"Symbols, qualified names, files, or search terms."},
                    "profile":{"type":"string","enum":["context","scout","auditor"],"default":"context"},
                    "depth":{"type":"integer","default":2,"minimum":1,"maximum":5},
                    "max_nodes":{"type":"integer","default":40,"minimum":1,"maximum":120},
                    "include":{"type":"array","items":{"type":"string","enum":["callers","callees","imports","services","tests","changes","coverage"]}},
                    "changed_paths":{"type":"array","items":{"type":"string"},"maxItems":40,"description":"Explicit changed files; avoids Git discovery."},
                    "expected_generation":{"type":"string","minLength":1},
                    "timeout_ms":{"type":"integer","default":3000,"minimum":1,"maximum":30000},
                    "include_source":{"type":"boolean","default":false,"description":"Include exact source for the most relevant callables (bounded, 6 nodes)."},
                    "max_output_tokens":{"type":"integer","default":3200,"minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: true,
        },
        ToolDef {
            name: "detect_changes",
            description: "Map a Git diff to files, changed symbols, impacted symbols and risk. Scope tests adds test impact and policy.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "scope":{"type":"string","enum":["files","impact","tests"],"default":"impact"},
                    "direction":{"type":"string","enum":["inbound","outbound","both"],"default":"inbound"},
                    "depth":{"type":"integer","default":2},
                    "edge_types":{"type":"array","items":{"type":"string"}},
                    "fixpoint":{"type":"boolean"},
                    "via":{"type":"boolean"},
                    "seeds":{"type":"string","enum":["none","list"],"default":"none"},
                    "seed_filter":{"type":"string","enum":["all","product"],"default":"all"},
                    "config_path":{"type":"string"},
                    "seed_limit":{"type":"integer","default":50,"minimum":0,"maximum":1000},
                    "seed_offset":{"type":"integer","default":0,"minimum":0},
                    "limit":{"type":"integer","default":200,"maximum":5000},
                    "impact_offset":{"type":"integer","default":0,"minimum":0},
                    "changed_limit":{"type":"integer","default":20,"minimum":0,"maximum":5000},
                    "changed_offset":{"type":"integer","default":0,"minimum":0},
                    "module_limit":{"type":"integer","default":20,"minimum":0,"maximum":256},
                    "module_offset":{"type":"integer","default":0,"minimum":0},
                    "max_output_tokens":{"type":"integer","default":3200,"minimum":128,"maximum":1000000},
                    "base_branch":{"type":"string"},
                    "since":{"type":"string"},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "manage_adr",
            description: "Outline an ADR by default; get reads it; update replaces it; set_sections rewrites only named sections byte-preservingly.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."},
                    "mode":{"type":"string","enum":["outline","get","update","set_sections","sections","store"],"default":"outline"},
                    "content":{"type":"string"},
                    "section_updates":{"type":"object","additionalProperties":{"type":"string"}},
                    "section_limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "section_offset":{"type":"integer","default":0,"minimum":0},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "additionalProperties":false,
                "required":["project"]
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "ingest_traces",
            description: "Record observed runtime calls so the graph can distinguish observed from merely possible paths. Only indexed endpoints are stored.",
            schema: json!({
                "type":"object",
                "properties":{
                    "traces":{"type":"array","items":{"type":"object","properties":{
                        "path":{"type":"array","items":{"type":"string"}},
                        "caller":{"type":"string"},
                        "callee":{"type":"string"},
                        "count":{"type":"integer"}
                    },"additionalProperties":false}},
                    "label":{"type":"string"},
                    "project":{"type":"string","description":"Optional; defaults to the indexed workspace root."}
                },
                "required":["traces","project"]
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: false,
            always_load: false,
        },
    ]
}

pub fn tool_visible(profile: ToolProfile, tool: &ToolDef) -> bool {
    match profile {
        ToolProfile::All => true,
        ToolProfile::Analysis => tool.read_only,
        ToolProfile::Scout => matches!(
            tool.name,
            "search_graph"
                | "get_task_context"
                | "trace_path"
                | "get_code_snippet"
                | "get_file_outline"
                | "get_architecture"
                | "search_code"
                | "get_graph_schema"
                | "check_index_coverage"
                | "list_projects"
                | "index_status"
        ),
    }
}

pub fn tool_json(tool: &ToolDef) -> Value {
    let mut out = json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": tool.schema,
        "annotations": {
            "title": tool.name,
            "readOnlyHint": tool.read_only,
            "destructiveHint": tool.destructive,
            "idempotentHint": tool.idempotent,
            "openWorldHint": tool.open_world,
        }
    });
    if tool.always_load {
        out["_meta"] = json!({"anthropic/alwaysLoad": true});
    }
    out
}

pub fn instructions(profile: ToolProfile) -> String {
    let base = "gry is a local code knowledge graph of the indexed repository: functions, classes, methods, routes, calls, imports, inheritance, documentation, configuration and infrastructure. Project arguments are optional; omit them to use the indexed workspace root. For a multi-part structural task, start with get_task_context (task, depth 3 for route-to-repository chains, include tests when the task asks about tests, impact or coverage). Use search_graph with name_pattern for a known identifier; query is broad BM25. trace_path follows callers/callees and returns Type.Method candidates when a short name is ambiguous. Use get_code_snippet or native reads for exact source before material claims; do not repeat discovery. Use search_code or native grep for exact literals and configuration. get_graph_schema is for authoring query_graph, not discovery; check_index_coverage is for a coverage doubt, not setup. Results are bounded and paginated; status, freshness, provenance and truncation are part of the evidence.";
    match profile {
        ToolProfile::All => base.to_string(),
        ToolProfile::Analysis => format!("analysis tool profile (read-only): {base}"),
        ToolProfile::Scout => format!(
            "scout tool profile (fast, provisional): {base} Narrow limits keep answers fast."
        ),
    }
}
