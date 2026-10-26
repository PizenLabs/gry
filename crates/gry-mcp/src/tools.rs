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
            description: "Find symbols via BM25 query, regex name/qn filters, or semantic_query. Rows keep qn/file/lines and in/out degree.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string"},
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
                "required":["project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: true,
        },
        ToolDef {
            name: "query_graph",
            description: "Read-only Cypher for multi-hop, aggregation, complexity, or cross-service analysis. Default 200 visible rows with exact totals and continuations.",
            schema: json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string"},
                    "project":{"type":"string"},
                    "graph":{"type":"string","enum":["code","missed"],"default":"code"},
                    "max_rows":{"type":"integer","minimum":0,"default":200},
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "cursor":{"type":"string"},
                    "max_output_tokens":{"type":"integer","minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["query","project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "trace_path",
            description: "Trace callers/callees, data flow, cross-service, topology, deploy or run paths. Depth 1-15; tests excluded by default.",
            schema: json!({
                "type":"object",
                "properties":{
                    "function_name":{"type":"string"},
                    "project":{"type":"string"},
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
                "required":["function_name","project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: true,
        },
        ToolDef {
            name: "get_code_snippet",
            description: "Read a search_graph symbol. auto bounds source and outlines large containers; full restores source.",
            schema: json!({
                "type":"object",
                "properties":{
                    "qualified_name":{"type":"string"},
                    "project":{"type":"string"},
                    "include_neighbors":{"type":"boolean","default":false},
                    "source_mode":{"type":"string","enum":["auto","full","outline"],"default":"auto"},
                    "member_limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "member_offset":{"type":"integer","default":0,"minimum":0},
                    "start_line":{"type":"integer","minimum":1},
                    "max_lines":{"type":"integer","minimum":1,"maximum":500},
                    "max_output_tokens":{"type":"integer","minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["qualified_name","project"]
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
                    "project":{"type":"string"},
                    "file_path":{"type":"string"},
                    "labels":{"type":"array","items":{"type":"string"},"maxItems":16},
                    "limit":{"type":"integer","minimum":1,"maximum":200,"default":100},
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "additionalProperties":false,
                "required":["project","file_path"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "get_graph_schema",
            description: "Get node-label and edge-type counts, relationship patterns and (diagnostics=full) queryable properties.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string"},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"},
                    "diagnostics":{"type":"string","enum":["none","full"],"default":"none"},
                    "limit":{"type":"integer","default":50,"minimum":1,"maximum":500},
                    "offset":{"type":"integer","default":0,"minimum":0}
                },
                "required":["project"]
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
            description: "Bounded static architecture: overview, structure, dependencies, routes, languages, packages, entry points, hotspots, boundaries, layers, file tree, clusters, cycles.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string"},
                    "path":{"type":"string","description":"Directory prefix."},
                    "aspects":{"type":"array","items":{"type":"string","enum":["all","overview","structure","dependencies","routes","languages","packages","entry_points","hotspots","boundaries","layers","file_tree","clusters","cycles","system_structure"]}},
                    "expected_generation":{"type":"string","minLength":1},
                    "max_output_tokens":{"type":"integer","default":8000,"minimum":128,"maximum":1000000},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "search_code",
            description: "Graph-ranked text search: compact symbols, full bounded source, or file paths.",
            schema: json!({
                "type":"object",
                "properties":{
                    "pattern":{"type":"string"},
                    "project":{"type":"string"},
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
                "required":["pattern","project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "list_projects",
            description: "List projects with stable paging. Identity is lean; detail=stats adds graph sizes.",
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
                "properties":{"project":{"type":"string"}},
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
            description: "Project readiness, counts, root, generation, coverage gaps and doc_links. Best-effort; verify cited paths.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string"},
                    "verbose":{"type":"boolean","default":false},
                    "diagnostics":{"type":"string","enum":["none","summary","full"],"default":"none"},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "check_index_coverage",
            description: "Best-effort exact-path/scope coverage and freshness, paged separately. Clean is not proof of completeness.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string"},
                    "paths":{"type":"array","items":{"type":"string"},"maxItems":128},
                    "path_limit":{"type":"integer","default":20,"minimum":1,"maximum":128},
                    "path_offset":{"type":"integer","default":0,"minimum":0},
                    "scopes":{"type":"array","items":{"type":"string"},"maxItems":32},
                    "scope_limit":{"type":"integer","default":20,"minimum":1,"maximum":1000},
                    "scope_offset":{"type":"integer","default":0,"minimum":0},
                    "diagnostics":{"type":"string","enum":["none","full"],"default":"none"},
                    "format":{"type":"string","enum":["tree","json"],"default":"tree"}
                },
                "required":["project"]
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
            always_load: false,
        },
        ToolDef {
            name: "detect_changes",
            description: "Map a Git diff to files, changed symbols, impacted symbols and risk. Scope tests adds test impact.",
            schema: json!({
                "type":"object",
                "properties":{
                    "project":{"type":"string"},
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
                "required":["project"]
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
                    "project":{"type":"string"},
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
                    "project":{"type":"string"}
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
    let base = "gry is a local code knowledge graph of the indexed repository: functions, classes, methods, routes, calls, imports, inheritance, documentation, configuration and infrastructure. It answers structural code questions: where a symbol is defined, who calls a function, what it calls, call chains and data flow, which code a change affects, how modules fit together, and how services and deployments connect. search_graph finds symbols; trace_path follows callers, callees, data flow and infrastructure paths; get_code_snippet returns exact source; query_graph runs multi-hop Cypher; get_architecture summarizes structure; search_code combines text search with graph context; detect_changes maps the current diff to affected symbols. check_index_coverage tells whether a cited file was fully indexed. Results are paginated.";
    match profile {
        ToolProfile::All => base.to_string(),
        ToolProfile::Analysis => format!("analysis tool profile (read-only): {base}"),
        ToolProfile::Scout => format!(
            "scout tool profile (fast, provisional): {base} Narrow limits keep answers fast."
        ),
    }
}
