//! MCP tool handlers: argument validation and dispatch to domain services.

use crate::tools::ToolProfile;
use gry_core::config::Config;
use gry_index::jobs::JobManager;
use gry_index::pipeline::{IndexRequest, Indexer};
use gry_query::adr::{manage_adr, AdrRequest};
use gry_query::architecture::{get_architecture, ArchitectureRequest};
use gry_query::compare::compare_graphs;
use gry_query::context::{get_task_context, ContextRequest};
use gry_query::coverage::{check_index_coverage, index_status};
use gry_query::cypher::{query_graph, QueryRequest};
use gry_query::impact::{detect_changes, DetectChangesRequest};
use gry_query::schema::get_graph_schema;
use gry_query::search::{
    get_code_snippet, get_file_outline, search_code, search_graph, GraphSearchRequest,
    OutlineRequest, SearchCodeRequest, SnippetRequest,
};
use gry_query::trace::{trace_path, TraceRequest};
use gry_query::traces::{ingest_traces, IngestRequest};
use gry_store::Store;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};

pub struct Ctx<'a> {
    pub config: &'a Config,
    pub jobs: &'a JobManager,
    pub last_project: &'a Mutex<Option<String>>,
    pub roots: &'a [String],
    pub profile: ToolProfile,
}

impl<'a> Ctx<'a> {
    fn store(&self) -> gry_core::GryResult<Store> {
        Store::open(&self.config.db_path)
    }

    fn project(&self, args: &Map<String, Value>) -> gry_core::GryResult<String> {
        if let Some(project) = args.get("project").and_then(Value::as_str) {
            if !project.is_empty() {
                return Ok(project.to_string());
            }
        }
        if let Some(project) = self.last_project.lock().clone() {
            return Ok(project);
        }
        let store = self.store()?;
        for root in self.roots {
            if let Ok(Some(record)) = store.project_by_root(root) {
                return Ok(record.name);
            }
            if let Ok(canonical) = std::path::Path::new(root).canonicalize() {
                if let Ok(Some(record)) = store.project_by_root(&canonical.to_string_lossy()) {
                    return Ok(record.name);
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(Some(record)) = store.project_by_root(&cwd.to_string_lossy()) {
                return Ok(record.name);
            }
        }
        let projects = store.projects()?;
        if projects.len() == 1 {
            return Ok(projects[0].name.clone());
        }
        Err(gry_core::GryError::invalid(
            "project is required; call list_projects to see indexed projects",
        ))
    }

    fn remember(&self, project: &str) {
        *self.last_project.lock() = Some(project.to_string());
    }
}

fn s(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn required_s(args: &Map<String, Value>, key: &str) -> gry_core::GryResult<String> {
    s(args, key)
        .ok_or_else(|| gry_core::GryError::invalid(format!("missing required argument: {key}")))
}

fn b(args: &Map<String, Value>, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn i(args: &Map<String, Value>, key: &str, default: i64) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn u(args: &Map<String, Value>, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn arr_strings(args: &Map<String, Value>, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn format(args: &Map<String, Value>) -> String {
    s(args, "format").unwrap_or_else(|| "tree".to_string())
}

fn max_tokens(args: &Map<String, Value>, default: usize) -> usize {
    u(args, "max_output_tokens", default)
}

pub fn dispatch(
    ctx: &Ctx<'_>,
    name: &str,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    match name {
        "index_repository" => index_repository(ctx, args),
        "list_projects" => list_projects(ctx, args),
        "delete_project" => delete_project(ctx, args),
        "index_status" => index_status_tool(ctx, args),
        "check_index_coverage" => coverage_tool(ctx, args),
        "search_graph" => search_graph_tool(ctx, args),
        "query_graph" => query_graph_tool(ctx, args),
        "trace_path" | "trace_call_path" => trace_tool(ctx, args),
        "get_code_snippet" => snippet_tool(ctx, args),
        "get_file_outline" => outline_tool(ctx, args),
        "get_graph_schema" => schema_tool(ctx, args),
        "compare_graphs" => compare_tool(ctx, args),
        "get_architecture" => architecture_tool(ctx, args),
        "search_code" => search_code_tool(ctx, args),
        "get_task_context" => context_tool(ctx, args),
        "detect_changes" => changes_tool(ctx, args),
        "manage_adr" => adr_tool(ctx, args),
        "ingest_traces" => traces_tool(ctx, args),
        other => Err(gry_core::GryError::invalid(format!(
            "unknown tool: {other}"
        ))),
    }
}

fn index_repository(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let repo_path = s(args, "repo_path");
    let requested_name = s(args, "name");
    if b(args, "status") {
        let project = if let Some(name) = &requested_name {
            name.clone()
        } else if let Some(repo) = &repo_path {
            let root = ctx.config.resolve_index_root(repo)?;
            let store = ctx.store()?;
            store
                .project_by_root(&root.to_string_lossy())?
                .map(|p| p.name)
                .unwrap_or_else(|| {
                    gry_core::fqn::project_name_from_path(root.to_string_lossy().as_ref())
                })
        } else {
            return Err(gry_core::GryError::invalid(
                "repo_path or name is required for status",
            ));
        };
        if let Some(status) = ctx.jobs.status(&project) {
            return Ok((serde_json::to_value(status)?, format(args), 0));
        }
        let store = ctx.store()?;
        let record = store
            .project(&project)?
            .ok_or_else(|| gry_core::GryError::ProjectNotFound(project.clone()))?;
        let value = json!({
            "project": project,
            "state": "succeeded",
            "started_at": record.indexed_at,
            "finished_at": record.indexed_at,
            "error": Value::Null,
            "generation": record.generation,
            "files": record.file_count,
            "nodes": store.counts(&project)?.0,
            "edges": store.counts(&project)?.1,
        });
        return Ok((value, format(args), 0));
    }
    let repo_path = repo_path.ok_or_else(|| gry_core::GryError::invalid("missing repo_path"))?;
    let mode = s(args, "mode").unwrap_or_else(|| "full".to_string());
    if !matches!(
        mode.as_str(),
        "full" | "moderate" | "fast" | "cross-repo-intelligence"
    ) {
        return Err(gry_core::GryError::invalid(format!(
            "invalid mode: {mode} (use full, moderate, fast, cross-repo-intelligence)"
        )));
    }
    let request = IndexRequest {
        repo_path,
        mode,
        name: requested_name,
        persistence: b(args, "persistence"),
        doc_candidates: args.get("doc_candidates").and_then(Value::as_bool),
        target_projects: arr_strings(args, "target_projects"),
    };
    if b(args, "async") {
        let status = ctx.jobs.start(ctx.config.clone(), request)?;
        ctx.remember(&status.project);
        return Ok((serde_json::to_value(status)?, format(args), 0));
    }
    let indexer = Indexer::new(ctx.config.clone());
    let report = indexer.index(&request)?;
    ctx.remember(&report.project);
    Ok((
        serde_json::to_value(&report)?,
        format(args),
        max_tokens(args, 3200),
    ))
}

fn list_projects(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let store = ctx.store()?;
    let stats = s(args, "detail").as_deref() == Some("stats") || b(args, "include_details");
    let limit = u(args, "limit", 50).clamp(1, 500);
    let offset = u(args, "offset", 0);
    let (projects, total) = store.list_projects(limit, offset, stats)?;
    let value = json!({
        "projects": projects,
        "total": total,
        "limit": limit,
        "offset": offset,
        "has_more": offset + limit < total as usize,
        "next_offset": if offset + limit < total as usize { Some(offset + limit) } else { None },
    });
    Ok((value, format(args), 0))
}

fn delete_project(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let mut store = ctx.store()?;
    if store.delete_project(&project)? {
        if ctx.last_project.lock().as_deref() == Some(&project) {
            *ctx.last_project.lock() = None;
        }
        Ok((
            json!({"project": project, "status": "deleted"}),
            format(args),
            0,
        ))
    } else {
        Err(gry_core::GryError::ProjectNotFound(project))
    }
}

fn index_status_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let verbose = b(args, "verbose");
    let diagnostics = s(args, "diagnostics").unwrap_or_else(|| "none".to_string());
    let value = index_status(&store, &project, verbose, &diagnostics)?;
    Ok((value, format(args), 0))
}

fn coverage_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let value = check_index_coverage(
        &store,
        &project,
        &arr_strings(args, "paths"),
        &arr_strings(args, "scopes"),
        u(args, "path_limit", 20).clamp(1, 128),
        u(args, "path_offset", 0),
        u(args, "scope_limit", 20).clamp(1, 1000),
        u(args, "scope_offset", 0),
        &s(args, "diagnostics").unwrap_or_else(|| "none".to_string()),
    )?;
    Ok((value, format(args), 0))
}

fn search_graph_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    ctx.remember(&project);
    let store = ctx.store()?;
    let detail_ids = s(args, "detail").as_deref() == Some("ids");
    let req = GraphSearchRequest {
        project,
        query: s(args, "query"),
        label: s(args, "label"),
        name_pattern: s(args, "name_pattern"),
        qn_pattern: s(args, "qn_pattern"),
        file_pattern: s(args, "file_pattern"),
        relationship: s(args, "relationship"),
        min_degree: args.get("min_degree").and_then(Value::as_i64),
        max_degree: args.get("max_degree").and_then(Value::as_i64),
        exclude_entry_points: b(args, "exclude_entry_points"),
        include_connected: b(args, "include_connected"),
        kind: s(args, "kind"),
        namespace: s(args, "namespace"),
        semantic_query: match args.get("semantic_query") {
            None => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            ),
            Some(_) => {
                return Err(gry_core::GryError::invalid(
                    "semantic_query must be an array of keyword strings",
                ))
            }
        },
        semantic_limit: u(args, "semantic_limit", 50).min(500),
        semantic_offset: u(args, "semantic_offset", 0).min(99_998),
        limit: u(args, "limit", 50).clamp(1, 500),
        offset: u(args, "offset", 0),
        fields: arr_strings(args, "fields"),
        detail_ids,
    };
    let value = search_graph(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 3200)))
}

fn query_graph_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    ctx.remember(&project);
    let store = ctx.store()?;
    let max_rows = if i(args, "max_rows", 200) == 0 {
        99_998
    } else {
        i(args, "max_rows", 200).clamp(1, 99_998) as usize
    };
    let req = QueryRequest {
        project,
        query: required_s(args, "query")?,
        max_rows,
        offset: u(args, "offset", 0),
        cursor: s(args, "cursor"),
        graph: s(args, "graph").unwrap_or_else(|| "code".to_string()),
    };
    let value = query_graph(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 3200)))
}

fn trace_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    ctx.remember(&project);
    let store = ctx.store()?;
    let req = TraceRequest {
        project,
        function_name: required_s(args, "function_name")?,
        direction: s(args, "direction").unwrap_or_else(|| "both".to_string()),
        depth: u(args, "depth", 3).clamp(1, 15),
        limit: u(args, "limit", 100).clamp(1, 5000),
        mode: s(args, "mode").unwrap_or_else(|| "calls".to_string()),
        parameter_name: s(args, "parameter_name"),
        edge_types: arr_strings(args, "edge_types"),
        risk_labels: b(args, "risk_labels"),
        include_tests: b(args, "include_tests"),
        include_evidence: b(args, "include_evidence"),
        cursor: s(args, "cursor"),
    };
    let value = trace_path(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 3200)))
}

fn snippet_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let req = SnippetRequest {
        project,
        qualified_name: required_s(args, "qualified_name")?,
        include_neighbors: b(args, "include_neighbors"),
        source_mode: s(args, "source_mode").unwrap_or_else(|| "auto".to_string()),
        member_limit: u(args, "member_limit", 50).clamp(1, 500),
        member_offset: u(args, "member_offset", 0),
        start_line: args
            .get("start_line")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        max_lines: args
            .get("max_lines")
            .and_then(Value::as_u64)
            .map(|v| v as usize),
    };
    let value = get_code_snippet(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 2500)))
}

fn outline_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let req = OutlineRequest {
        project,
        file_path: required_s(args, "file_path")?,
        labels: arr_strings(args, "labels"),
        limit: u(args, "limit", 100).clamp(1, 200),
        offset: u(args, "offset", 0),
    };
    let value = get_file_outline(&store, &req)?;
    Ok((value, format(args), 0))
}

fn schema_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let value = get_graph_schema(
        &store,
        &project,
        &s(args, "diagnostics").unwrap_or_else(|| "none".to_string()),
        u(args, "limit", 50).clamp(1, 500),
        u(args, "offset", 0),
    )?;
    Ok((value, format(args), 0))
}

fn compare_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let store = ctx.store()?;
    let value = compare_graphs(
        &store,
        &required_s(args, "base_project")?,
        &required_s(args, "target_project")?,
        u(args, "limit", 200).clamp(1, 1000),
        u(args, "scan_limit", 2_000_000).clamp(1, 10_000_000),
    )?;
    Ok((value, format(args), 0))
}

fn architecture_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let aspects = arr_strings(args, "aspects");
    let req = ArchitectureRequest {
        project,
        path: s(args, "path"),
        aspects,
        expected_generation: s(args, "expected_generation"),
    };
    let value = get_architecture(&store, &req)?;
    // Architecture reports are naturally larger than symbol queries, and the
    // default aspect set already exceeds the 3200-token budget on mid-sized
    // repositories. Keep a workable default and let callers raise it.
    Ok((value, format(args), max_tokens(args, 8000)))
}

fn search_code_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let req = SearchCodeRequest {
        project,
        pattern: required_s(args, "pattern")?,
        file_pattern: s(args, "file_pattern"),
        path_filter: s(args, "path_filter"),
        mode: s(args, "mode").unwrap_or_else(|| "compact".to_string()),
        context: u(args, "context", 0).min(200),
        regex: b(args, "regex"),
        debug: b(args, "debug"),
        result_limit: u(args, "result_limit", u(args, "limit", 10)).clamp(1, 500),
        result_offset: u(args, "result_offset", 0),
        raw_limit: u(args, "raw_limit", 5).min(100),
        raw_offset: u(args, "raw_offset", 0),
        directory_limit: u(args, "directory_limit", 20).min(64),
        directory_offset: u(args, "directory_offset", 0),
        match_limit: u(args, "match_limit", 8).clamp(1, 500),
        source_max_lines: u(args, "source_max_lines", 20).clamp(1, 200),
        cursor: s(args, "cursor"),
    };
    let value = search_code(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 3200)))
}

fn context_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    ctx.remember(&project);
    let store = ctx.store()?;
    let req = ContextRequest {
        project,
        task: s(args, "task"),
        targets: arr_strings(args, "targets"),
        profile: s(args, "profile").unwrap_or_else(|| "context".to_string()),
        depth: u(args, "depth", 2).clamp(1, 5),
        max_nodes: u(args, "max_nodes", 40).clamp(1, 120),
        max_tokens: max_tokens(args, 3200).clamp(128, 1_000_000),
        include: arr_strings(args, "include"),
        expected_generation: s(args, "expected_generation"),
        changed_paths: arr_strings(args, "changed_paths"),
        timeout_ms: u(args, "timeout_ms", 3000).clamp(1, 30_000) as u64,
        task_id: None,
        stage: None,
        include_source: b(args, "include_source"),
    };
    let value = get_task_context(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 3200)))
}

fn changes_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let req = DetectChangesRequest {
        project,
        scope: s(args, "scope").unwrap_or_else(|| "impact".to_string()),
        direction: s(args, "direction").unwrap_or_else(|| "inbound".to_string()),
        depth: u(args, "depth", 2),
        edge_types: arr_strings(args, "edge_types"),
        fixpoint: b(args, "fixpoint"),
        via: b(args, "via"),
        seeds: b(args, "seeds"),
        seed_filter: s(args, "seed_filter").unwrap_or_else(|| "all".to_string()),
        base_branch: s(args, "base_branch"),
        since: s(args, "since"),
        limit: u(args, "limit", 200).min(5000),
        impact_offset: u(args, "impact_offset", 0),
        changed_limit: u(args, "changed_limit", 20).min(5000),
        changed_offset: u(args, "changed_offset", 0),
        module_limit: u(args, "module_limit", 20).min(256),
        module_offset: u(args, "module_offset", 0),
        seed_limit: u(args, "seed_limit", 50).min(1000),
        seed_offset: u(args, "seed_offset", 0),
        config_path: s(args, "config_path"),
    };
    let value = detect_changes(&store, &req)?;
    Ok((value, format(args), max_tokens(args, 3200)))
}

fn adr_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let updates = args
        .get("section_updates")
        .and_then(Value::as_object)
        .cloned();
    let req = AdrRequest {
        project,
        mode: s(args, "mode").unwrap_or_else(|| "outline".to_string()),
        content: args
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        section_updates: updates,
        section_limit: u(args, "section_limit", 50).clamp(1, 500),
        section_offset: u(args, "section_offset", 0),
    };
    let value = manage_adr(&store, &req)?;
    Ok((value, format(args), 0))
}

fn traces_tool(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
) -> gry_core::GryResult<(Value, String, usize)> {
    let project = ctx.project(args)?;
    let store = ctx.store()?;
    let traces = args
        .get("traces")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| gry_core::GryError::invalid("missing required argument: traces"))?;
    let value = ingest_traces(
        &store,
        &IngestRequest {
            project,
            label: s(args, "label"),
            traces,
        },
    )?;
    Ok((value, format(args), 0))
}
