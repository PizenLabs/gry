use clap::{Args, Parser, Subcommand};
use gry_core::config::{all_settings, get_setting, reset_setting, set_setting, Config};
use gry_index::discover::IndexMode;
use gry_index::jobs::JobManager;
use gry_index::pipeline::{read_job_status, write_job_status, IndexRequest, Indexer};
use gry_mcp::handlers::{dispatch, Ctx};
use gry_mcp::render::render;
use gry_mcp::{Server, ToolProfile};
use gry_query::test_impact::{publish, select_tests, PublishRequest, SelectRequest};
use gry_store::Store;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);
static QUIET_OUTPUT: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    name = "gry",
    version,
    about = "gry — local-first code intelligence for AI agents",
    long_about = "gry indexes a repository into a persistent knowledge graph and serves it over MCP or the CLI."
)]
struct Cli {
    /// SQLite database path (overrides GRY_DB).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Cache directory (overrides GRY_CACHE_DIR).
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    /// Errors only.
    #[arg(long, global = true)]
    quiet: bool,
    /// Informational logs.
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index a repository.
    Index(IndexCmd),
    /// Show index status.
    Status(StatusCmd),
    /// List indexed projects.
    Projects(ProjectsCmd),
    /// Delete a project and its graph.
    Delete { project: String },
    /// Search symbols.
    Search(SearchCmd),
    /// Search source text with graph context.
    SearchCode(SearchCodeCmd),
    /// Trace callers/callees/paths.
    Trace(TraceCmd),
    /// Read a symbol's source.
    Snippet(SnippetCmd),
    /// Outline a file.
    Outline(OutlineCmd),
    /// Show graph schema.
    Schema {
        project: String,
        #[arg(long, default_value = "none")]
        diagnostics: String,
    },
    /// Run a read-only Cypher query.
    Query(QueryCmd),
    /// Summarize architecture.
    Architecture(ArchitectureCmd),
    /// Check coverage.
    Coverage(CoverageCmd),
    /// Detect change impact.
    Changes(ChangesCmd),
    /// Manage the ADR.
    Adr(AdrCmd),
    /// Ingest runtime traces.
    Traces(TracesCmd),
    /// Compare two indexed snapshots.
    Compare {
        base: String,
        target: String,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// Run the MCP server over stdio.
    Serve(ServeCmd),
    /// Invoke any MCP tool once.
    Cli(CliCmd),
    /// Watch a repository and re-index on change.
    Watch(WatchCmd),
    /// Test-impact selection and publishing.
    TestImpact(TestImpactCmd),
    /// Read or change runtime settings.
    Config(ConfigCmd),
    /// Configure MCP clients.
    Install(InstallCmd),
    /// Remove gry from an MCP client configuration.
    Uninstall(UninstallCmd),
    /// Print or install an agent skill.
    Skill(SkillCmd),
    /// Show how to update this installation.
    Update,
    /// Check cache, database, FTS and build health.
    Doctor,
    #[command(name = "__index-worker", hide = true)]
    IndexWorker(WorkerCmd),
}

#[derive(Args)]
struct IndexCmd {
    repo: String,
    #[arg(long, default_value = "full")]
    mode: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    persistence: bool,
    #[arg(long)]
    r#async: bool,
    #[arg(long)]
    status: bool,
    #[arg(long = "target")]
    targets: Vec<String>,
}

#[derive(Args)]
struct StatusCmd {
    project: Option<String>,
    #[arg(long)]
    verbose: bool,
    #[arg(long, default_value = "none")]
    diagnostics: String,
}

#[derive(Args)]
struct ProjectsCmd {
    #[arg(long)]
    stats: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
}

#[derive(Args)]
struct SearchCmd {
    project: String,
    query: String,
    #[arg(long)]
    label: Option<String>,
    #[arg(long = "name-pattern")]
    name_pattern: Option<String>,
    #[arg(long = "qn-pattern")]
    qn_pattern: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
    #[arg(long = "semantic")]
    semantic: Vec<String>,
}

#[derive(Args)]
struct SearchCodeCmd {
    project: String,
    pattern: String,
    #[arg(long, default_value = "compact")]
    mode: String,
    #[arg(long)]
    regex: bool,
    #[arg(long, default_value_t = 0)]
    context: usize,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
    #[arg(long = "file-pattern")]
    file_pattern: Option<String>,
}

#[derive(Args)]
struct TraceCmd {
    project: String,
    function: String,
    #[arg(long, default_value = "both")]
    direction: String,
    #[arg(long, default_value_t = 3)]
    depth: usize,
    #[arg(long, default_value = "calls")]
    mode: String,
    #[arg(long)]
    include_tests: bool,
    #[arg(long)]
    risk: bool,
    #[arg(long, default_value_t = 100)]
    limit: usize,
}

#[derive(Args)]
struct SnippetCmd {
    project: String,
    qualified_name: String,
    #[arg(long, default_value = "auto")]
    source_mode: String,
    #[arg(long)]
    neighbors: bool,
    #[arg(long)]
    start_line: Option<i32>,
    #[arg(long)]
    max_lines: Option<usize>,
}

#[derive(Args)]
struct OutlineCmd {
    project: String,
    file: String,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[arg(long, default_value_t = 100)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
}

#[derive(Args)]
struct QueryCmd {
    project: String,
    query: String,
    #[arg(long, default_value_t = 200)]
    max_rows: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
}

#[derive(Args)]
struct ArchitectureCmd {
    project: String,
    #[arg(long = "aspect")]
    aspects: Vec<String>,
    #[arg(long)]
    path: Option<String>,
}

#[derive(Args)]
struct CoverageCmd {
    project: String,
    #[arg(long = "path")]
    paths: Vec<String>,
    #[arg(long = "scope")]
    scopes: Vec<String>,
    #[arg(long, default_value = "none")]
    diagnostics: String,
}

#[derive(Args)]
struct ChangesCmd {
    project: String,
    #[arg(long, default_value = "impact")]
    scope: String,
    #[arg(long, default_value = "inbound")]
    direction: String,
    #[arg(long, default_value_t = 2)]
    depth: usize,
    #[arg(long)]
    fixpoint: bool,
    #[arg(long)]
    base_branch: Option<String>,
    #[arg(long)]
    since: Option<String>,
}

#[derive(Args)]
struct AdrCmd {
    project: String,
    #[arg(long, default_value = "outline")]
    mode: String,
    #[arg(long)]
    content: Option<String>,
    #[arg(long = "section", value_parser = parse_section)]
    sections: Vec<(String, String)>,
}

fn parse_section(raw: &str) -> Result<(String, String), String> {
    raw.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| "section must be NAME=CONTENT".to_string())
}

#[derive(Args)]
struct TracesCmd {
    project: String,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    label: Option<String>,
}

#[derive(Args)]
struct ServeCmd {
    #[arg(long, default_value = "all")]
    profile: String,
}

#[derive(Args)]
struct CliCmd {
    tool: String,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    rest: Vec<String>,
}

#[derive(Args)]
struct WatchCmd {
    repo: String,
    #[arg(long, default_value_t = 2)]
    interval: u64,
    #[arg(long, default_value = "full")]
    mode: String,
}

#[derive(Args)]
struct TestImpactCmd {
    #[command(subcommand)]
    command: TestImpactSub,
}

#[derive(Subcommand)]
enum TestImpactSub {
    Select(TestImpactSelect),
    Publish(TestImpactPublish),
}

#[derive(Args)]
struct TestImpactSelect {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    config: Option<String>,
    #[arg(long)]
    coverage: Option<String>,
    #[arg(long, default_value = "linux-x86_64")]
    platform: String,
}

#[derive(Args)]
struct TestImpactPublish {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    out: String,
    #[arg(long)]
    coverage: Option<String>,
    #[arg(long, default_value = "linux-x86_64")]
    platform: String,
}

#[derive(Args)]
struct ConfigCmd {
    #[command(subcommand)]
    command: ConfigSub,
}

#[derive(Subcommand)]
enum ConfigSub {
    List,
    Get { key: String },
    Set { key: String, value: String },
    Reset { key: String },
}

#[derive(Args)]
struct InstallCmd {
    #[arg(long, default_value = "claude")]
    client: String,
    #[arg(long)]
    list_clients: bool,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    skill: bool,
}

#[derive(Args)]
struct UninstallCmd {
    #[arg(long, default_value = "claude")]
    client: String,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct SkillCmd {
    #[arg(long)]
    install: bool,
}

#[derive(Args)]
struct WorkerCmd {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, default_value = "full")]
    mode: String,
    #[arg(long)]
    persistence: bool,
}

fn main() {
    let cli = Cli::parse();
    if let Some(db) = &cli.db {
        std::env::set_var("GRY_DB", db);
    }
    if let Some(cache) = &cli.cache_dir {
        std::env::set_var("GRY_CACHE_DIR", cache);
    }
    JSON_OUTPUT.store(cli.json, Ordering::Relaxed);
    QUIET_OUTPUT.store(cli.quiet, Ordering::Relaxed);
    if cli.verbose {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("info")
            .with_writer(std::io::stderr)
            .try_init();
    }
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gry: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = run(&cli, &config) {
        eprintln!("gry: {e}");
        std::process::exit(1);
    }
}

fn run(cli: &Cli, config: &Config) -> gry_core::GryResult<()> {
    match &cli.command {
        Command::Index(cmd) => index_cmd(config, cmd),
        Command::Status(cmd) => status_cmd(config, cmd, cli.json),
        Command::Projects(cmd) => {
            let store = open_store(config)?;
            let (projects, total) = store.list_projects(cmd.limit, cmd.offset, cmd.stats)?;
            print_value(&json!({
                "projects": projects,
                "total": total,
                "has_more": cmd.offset + cmd.limit < total as usize,
            }));
            Ok(())
        }
        Command::Delete { project } => {
            if let Some(status) = read_job_status(config, project) {
                if matches!(
                    status.get("state").and_then(Value::as_str).unwrap_or(""),
                    "queued" | "running" | "cancelling"
                ) {
                    return Err(gry_core::GryError::Busy(format!(
                        "project {project} is being indexed; wait for the job to finish"
                    )));
                }
            }
            let mut store = open_store(config)?;
            if store.delete_project(project)? {
                print_value(&json!({"project": project, "status": "deleted"}));
                Ok(())
            } else {
                Err(gry_core::GryError::ProjectNotFound(project.clone()))
            }
        }
        Command::Search(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::search::GraphSearchRequest {
                project: cmd.project.clone(),
                query: Some(cmd.query.clone()),
                label: cmd.label.clone(),
                name_pattern: cmd.name_pattern.clone(),
                qn_pattern: cmd.qn_pattern.clone(),
                semantic_query: if cmd.semantic.is_empty() {
                    None
                } else {
                    Some(cmd.semantic.clone())
                },
                limit: cmd.limit,
                offset: cmd.offset,
                ..Default::default()
            };
            print_value(&gry_query::search::search_graph(&store, &req)?);
            Ok(())
        }
        Command::SearchCode(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::search::SearchCodeRequest {
                project: cmd.project.clone(),
                pattern: cmd.pattern.clone(),
                file_pattern: cmd.file_pattern.clone(),
                mode: cmd.mode.clone(),
                regex: cmd.regex,
                context: cmd.context,
                result_limit: cmd.limit,
                result_offset: cmd.offset,
                ..Default::default()
            };
            print_value(&gry_query::search::search_code(&store, &req)?);
            Ok(())
        }
        Command::Trace(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::trace::TraceRequest {
                project: cmd.project.clone(),
                function_name: cmd.function.clone(),
                direction: cmd.direction.clone(),
                depth: cmd.depth,
                limit: cmd.limit,
                mode: cmd.mode.clone(),
                include_tests: cmd.include_tests,
                risk_labels: cmd.risk,
                ..Default::default()
            };
            print_value(&gry_query::trace::trace_path(&store, &req)?);
            Ok(())
        }
        Command::Snippet(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::search::SnippetRequest {
                project: cmd.project.clone(),
                qualified_name: cmd.qualified_name.clone(),
                include_neighbors: cmd.neighbors,
                source_mode: cmd.source_mode.clone(),
                member_limit: 50,
                member_offset: 0,
                start_line: cmd.start_line,
                max_lines: cmd.max_lines,
            };
            print_value(&gry_query::search::get_code_snippet(&store, &req)?);
            Ok(())
        }
        Command::Outline(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::search::OutlineRequest {
                project: cmd.project.clone(),
                file_path: cmd.file.clone(),
                labels: cmd.labels.clone(),
                limit: cmd.limit,
                offset: cmd.offset,
            };
            print_value(&gry_query::search::get_file_outline(&store, &req)?);
            Ok(())
        }
        Command::Schema {
            project,
            diagnostics,
        } => {
            let store = open_store(config)?;
            print_value(&gry_query::schema::get_graph_schema(
                &store,
                project,
                diagnostics,
                50,
                0,
            )?);
            Ok(())
        }
        Command::Query(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::cypher::QueryRequest {
                project: cmd.project.clone(),
                query: cmd.query.clone(),
                max_rows: cmd.max_rows,
                offset: cmd.offset,
                cursor: None,
                graph: "code".to_string(),
            };
            print_value(&gry_query::cypher::query_graph(&store, &req)?);
            Ok(())
        }
        Command::Architecture(cmd) => {
            let store = open_store(config)?;
            print_value(&gry_query::architecture::get_architecture(
                &store,
                &gry_query::architecture::ArchitectureRequest {
                    project: cmd.project.clone(),
                    path: cmd.path.clone(),
                    aspects: cmd.aspects.clone(),
                    expected_generation: None,
                },
            )?);
            Ok(())
        }
        Command::Coverage(cmd) => {
            let store = open_store(config)?;
            print_value(&gry_query::coverage::check_index_coverage(
                &store,
                &cmd.project,
                &cmd.paths,
                &cmd.scopes,
                20,
                0,
                20,
                0,
                &cmd.diagnostics,
            )?);
            Ok(())
        }
        Command::Changes(cmd) => {
            let store = open_store(config)?;
            let req = gry_query::impact::DetectChangesRequest {
                project: cmd.project.clone(),
                scope: cmd.scope.clone(),
                direction: cmd.direction.clone(),
                depth: cmd.depth,
                fixpoint: cmd.fixpoint,
                base_branch: cmd.base_branch.clone(),
                since: cmd.since.clone(),
                ..Default::default()
            };
            print_value(&gry_query::impact::detect_changes(&store, &req)?);
            Ok(())
        }
        Command::Adr(cmd) => {
            let store = open_store(config)?;
            let mut updates = Map::new();
            for (k, v) in &cmd.sections {
                updates.insert(k.clone(), Value::String(v.clone()));
            }
            let req = gry_query::adr::AdrRequest {
                project: cmd.project.clone(),
                mode: cmd.mode.clone(),
                content: cmd.content.clone(),
                section_updates: if updates.is_empty() {
                    None
                } else {
                    Some(updates)
                },
                section_limit: 50,
                section_offset: 0,
            };
            print_value(&gry_query::adr::manage_adr(&store, &req)?);
            Ok(())
        }
        Command::Traces(cmd) => {
            let store = open_store(config)?;
            let traces: Value = match &cmd.file {
                Some(path) => serde_json::from_slice(&std::fs::read(path)?)?,
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    serde_json::from_str(&buf)?
                }
            };
            let traces = traces
                .get("traces")
                .and_then(Value::as_array)
                .cloned()
                .or_else(|| traces.as_array().cloned())
                .unwrap_or_default();
            print_value(&gry_query::traces::ingest_traces(
                &store,
                &gry_query::traces::IngestRequest {
                    project: cmd.project.clone(),
                    label: cmd.label.clone(),
                    traces,
                },
            )?);
            Ok(())
        }
        Command::Compare {
            base,
            target,
            limit,
        } => {
            let store = open_store(config)?;
            print_value(&gry_query::compare::compare_graphs(
                &store, base, target, *limit, 2_000_000,
            )?);
            Ok(())
        }
        Command::Serve(cmd) => {
            let profile = ToolProfile::parse(&cmd.profile).ok_or_else(|| {
                gry_core::GryError::invalid("profile must be all, analysis or scout")
            })?;
            let mut server = Server::new(config.clone(), profile);
            server.serve_stdio()
        }
        Command::Cli(cmd) => cli_tool(config, cmd, cli.json),
        Command::Watch(cmd) => {
            let mode = IndexMode::parse(&cmd.mode)
                .ok_or_else(|| gry_core::GryError::invalid("invalid mode"))?;
            gry_index::watcher::watch(
                config.clone(),
                gry_index::watcher::WatchOptions {
                    repo: cmd.repo.clone(),
                    interval_secs: cmd.interval,
                    mode,
                },
            )
        }
        Command::TestImpact(cmd) => test_impact_cmd(config, cmd),
        Command::Config(cmd) => config_cmd(config, cmd),
        Command::Install(cmd) => install_cmd(config, cmd),
        Command::Uninstall(cmd) => uninstall_cmd(cmd),
        Command::Skill(cmd) => skill_cmd(cmd),
        Command::Doctor => doctor_cmd(config),
        Command::Update => {
            print_value(&json!({
                "version": env!("CARGO_PKG_VERSION"),
                "status": "manual_update",
                "message": "gry does not self-update. Rebuild from source or use your package manager.",
                "command": "cargo install --path crates/gry-cli --force"
            }));
            Ok(())
        }
        Command::IndexWorker(cmd) => worker_cmd(config, cmd),
    }
}

fn open_store(config: &Config) -> gry_core::GryResult<Store> {
    config.ensure_dirs()?;
    Store::open(&config.db_path)
}

fn print_value(value: &Value) {
    if QUIET_OUTPUT.load(Ordering::Relaxed) {
        return;
    }
    let text = if JSON_OUTPUT.load(Ordering::Relaxed) {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    };
    println!("{}", text.unwrap_or_else(|_| "{}".to_string()));
}

fn doctor_cmd(config: &Config) -> gry_core::GryResult<()> {
    let mut checks: Vec<Value> = Vec::new();
    let mut ok = true;
    match config.ensure_dirs().and_then(|_| {
        let path = config.cache_dir.join(".gry-doctor");
        std::fs::write(&path, b"ok")?;
        std::fs::remove_file(&path)?;
        Ok(())
    }) {
        Ok(()) => checks.push(json!({"check":"cache_writable","ok":true,"path":config.cache_dir})),
        Err(e) => {
            ok = false;
            checks.push(json!({"check":"cache_writable","ok":false,"error":e.to_string()}));
        }
    }
    match open_store(config) {
        Ok(store) => {
            checks.push(json!({"check":"database","ok":true,"path":config.db_path}));
            match store.integrity_check() {
                Ok(rows) if rows == ["ok"] => checks.push(json!({"check":"integrity","ok":true})),
                Ok(rows) => {
                    ok = false;
                    checks.push(json!({"check":"integrity","ok":false,"result":rows}));
                }
                Err(e) => {
                    ok = false;
                    checks.push(json!({"check":"integrity","ok":false,"error":e.to_string()}));
                }
            }
            match store.has_fts() {
                Ok(true) => checks.push(json!({"check":"fts5","ok":true})),
                Ok(false) => {
                    ok = false;
                    checks.push(json!({"check":"fts5","ok":false,"error":"nodes_fts missing"}));
                }
                Err(e) => {
                    ok = false;
                    checks.push(json!({"check":"fts5","ok":false,"error":e.to_string()}));
                }
            }
            checks.push(json!({
                "check":"projects",
                "ok":true,
                "count": store.projects().map(|p| p.len()).unwrap_or(0)
            }));
            checks.push(json!({
                "check":"format",
                "ok":true,
                "schema": gry_store::SCHEMA_VERSION,
                "index": gry_store::FORMAT_VERSION,
                "extractor": gry_index::pipeline::EXTRACTOR_VERSION
            }));
            checks.push(json!({
                "check":"delta",
                "ok": store.supports_delta(),
                "note": "FTS5 contentless-delete"
            }));
        }
        Err(e) => {
            ok = false;
            checks.push(json!({"check":"database","ok":false,"error":e.to_string()}));
        }
    }
    checks.push(json!({
        "check":"clients",
        "ok":true,
        "supported": SUPPORTED_CLIENTS.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    }));
    print_value(&json!({
        "ok": ok,
        "version": env!("CARGO_PKG_VERSION"),
        "cache_dir": config.cache_dir,
        "db_path": config.db_path,
        "checks": checks
    }));
    if ok {
        Ok(())
    } else {
        Err(gry_core::GryError::invalid("doctor found unhealthy checks"))
    }
}

fn index_cmd(config: &Config, cmd: &IndexCmd) -> gry_core::GryResult<()> {
    let mode = IndexMode::parse(&cmd.mode).ok_or_else(|| {
        gry_core::GryError::invalid("mode must be full, moderate, fast or cross-repo-intelligence")
    })?;
    let root = config.resolve_index_root(&cmd.repo)?;
    let project = cmd
        .name
        .as_deref()
        .map(gry_core::fqn::sanitize_project_name)
        .unwrap_or_else(|| gry_core::fqn::project_name_from_path(root.to_string_lossy().as_ref()));
    if cmd.status {
        if let Some(status) = read_job_status(config, &project) {
            let state = status.get("state").and_then(Value::as_str).unwrap_or("");
            if matches!(state, "queued" | "running" | "cancelling") {
                print_value(&status);
                return Ok(());
            }
        }
        let store = open_store(config)?;
        let record = store
            .project(&project)?
            .ok_or_else(|| gry_core::GryError::ProjectNotFound(project.clone()))?;
        print_value(&json!({
            "project": project,
            "state": "succeeded",
            "started_at": record.indexed_at,
            "finished_at": record.indexed_at,
            "generation": record.generation,
            "files": record.file_count,
        }));
        return Ok(());
    }
    let request = IndexRequest {
        repo_path: root.to_string_lossy().to_string(),
        mode: mode.as_str().to_string(),
        name: Some(project.clone()),
        persistence: cmd.persistence,
        doc_candidates: None,
        target_projects: cmd.targets.clone(),
    };
    if cmd.r#async {
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("__index-worker")
            .arg("--repo")
            .arg(&request.repo_path)
            .arg("--mode")
            .arg(&request.mode)
            .arg("--name")
            .arg(&project);
        if request.persistence {
            command.arg("--persistence");
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        let child = command.spawn()?;
        let status = json!({
            "project": project,
            "state": "queued",
            "pid": child.id(),
            "started_at": gry_store::now_iso(),
            "finished_at": Value::Null,
            "error": Value::Null,
        });
        write_job_status(config, &project, &status);
        print_value(&status);
        return Ok(());
    }
    let indexer = Indexer::new(config.clone());
    let report = indexer.index(&request)?;
    print_value(&serde_json::to_value(report)?);
    Ok(())
}

fn status_cmd(config: &Config, cmd: &StatusCmd, _json: bool) -> gry_core::GryResult<()> {
    let store = open_store(config)?;
    let project = match &cmd.project {
        Some(p) => p.clone(),
        None => {
            let projects = store.projects()?;
            if projects.len() == 1 {
                projects[0].name.clone()
            } else if let Ok(cwd) = std::env::current_dir() {
                store
                    .project_by_root(&cwd.to_string_lossy())?
                    .map(|p| p.name)
                    .ok_or_else(|| gry_core::GryError::invalid("project is required"))?
            } else {
                return Err(gry_core::GryError::invalid("project is required"));
            }
        }
    };
    if let Some(status) = read_job_status(config, &project) {
        let state = status.get("state").and_then(Value::as_str).unwrap_or("");
        if matches!(state, "queued" | "running" | "cancelling" | "failed") {
            print_value(&status);
            return Ok(());
        }
    }
    print_value(&gry_query::coverage::index_status(
        &store,
        &project,
        cmd.verbose,
        &cmd.diagnostics,
    )?);
    Ok(())
}

fn worker_cmd(config: &Config, cmd: &WorkerCmd) -> gry_core::GryResult<()> {
    let request = IndexRequest {
        repo_path: cmd.repo.clone(),
        mode: cmd.mode.clone(),
        name: cmd.name.clone(),
        persistence: cmd.persistence,
        doc_candidates: None,
        target_projects: Vec::new(),
    };
    let project = cmd
        .name
        .clone()
        .unwrap_or_else(|| gry_core::fqn::project_name_from_path(&cmd.repo));
    let status = json!({
        "project": project,
        "state": "running",
        "started_at": gry_store::now_iso(),
        "finished_at": Value::Null,
        "error": Value::Null,
    });
    write_job_status(config, &project, &status);
    let indexer = Indexer::new(config.clone());
    match indexer.index(&request) {
        Ok(report) => {
            write_job_status(
                config,
                &project,
                &json!({
                    "project": project,
                    "state": "succeeded",
                    "started_at": status["started_at"],
                    "finished_at": gry_store::now_iso(),
                    "error": Value::Null,
                    "report": report,
                }),
            );
        }
        Err(e) => {
            write_job_status(
                config,
                &project,
                &json!({
                    "project": project,
                    "state": "failed",
                    "started_at": status["started_at"],
                    "finished_at": gry_store::now_iso(),
                    "error": e.to_string(),
                }),
            );
        }
    }
    Ok(())
}

fn cli_tool(config: &Config, cmd: &CliCmd, outer_json: bool) -> gry_core::GryResult<()> {
    use std::io::IsTerminal;
    let tools = gry_mcp::tools::tools();
    let Some(tool) = tools.iter().find(|t| t.name == cmd.tool) else {
        return Err(gry_core::GryError::invalid(format!(
            "unknown tool: {}",
            cmd.tool
        )));
    };
    if cmd.rest.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.schema
            }))
            .unwrap()
        );
        return Ok(());
    }
    let mut args = parse_tool_args(tool.schema.clone(), &cmd.rest)?;
    // Accept --args-file / --args and piped JSON.
    if let Some(path) = args
        .remove("__args_file")
        .and_then(|v| v.as_str().map(str::to_string))
    {
        let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        if let Value::Object(map) = value {
            for (k, v) in map {
                args.insert(k, v);
            }
        }
    }
    if let Some(raw) = args
        .remove("__args")
        .and_then(|v| v.as_str().map(str::to_string))
    {
        let value: Value = serde_json::from_str(&raw)?;
        if let Value::Object(map) = value {
            for (k, v) in map {
                args.insert(k, v);
            }
        }
    }
    if args.is_empty() && !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        if !buf.trim().is_empty() {
            if let Ok(Value::Object(map)) = serde_json::from_str(&buf) {
                args = map;
            }
        }
    }
    if cmd.tool == "index_repository" && args.get("async").and_then(Value::as_bool).unwrap_or(false)
    {
        let repo = args
            .get("repo_path")
            .and_then(Value::as_str)
            .ok_or_else(|| gry_core::GryError::invalid("missing repo_path"))?;
        let mode = args.get("mode").and_then(Value::as_str).unwrap_or("full");
        let name = args.get("name").and_then(Value::as_str).map(str::to_string);
        let cmd = IndexCmd {
            repo: repo.to_string(),
            mode: mode.to_string(),
            name,
            persistence: args
                .get("persistence")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            r#async: true,
            status: false,
            targets: Vec::new(),
        };
        return index_cmd(config, &cmd);
    }
    let jobs = JobManager::new();
    let last = Mutex::new(None);
    let roots: Vec<String> = Vec::new();
    let ctx = Ctx {
        config,
        jobs: &jobs,
        last_project: &last,
        roots: &roots,
        profile: ToolProfile::All,
    };
    let (value, format, max_tokens) = dispatch(&ctx, &cmd.tool, &args)?;
    let text = render(
        &value,
        &format,
        if max_tokens == 0 { 3200 } else { max_tokens },
    );
    if outer_json {
        print_value(
            &json!({"tool": cmd.tool, "result": serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))}),
        );
    } else {
        println!("{text}");
    }
    Ok(())
}

fn parse_tool_args(schema: Value, rest: &[String]) -> gry_core::GryResult<Map<String, Value>> {
    use std::io::IsTerminal;
    let _ = std::io::stdin().is_terminal();
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut args = Map::new();
    let mut i = 0;
    while i < rest.len() {
        let raw = &rest[i];
        if !raw.starts_with("--") {
            i += 1;
            continue;
        }
        let (key, inline) = match raw[2..].split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (raw[2..].replace('-', "_"), None),
        };
        if key == "args_file" {
            if let Some(v) = inline {
                args.insert("__args_file".into(), Value::String(v));
            } else if i + 1 < rest.len() {
                args.insert("__args_file".into(), Value::String(rest[i + 1].clone()));
                i += 1;
            }
            i += 1;
            continue;
        }
        if key == "args" {
            if let Some(v) = inline {
                args.insert("__args".into(), Value::String(v));
            } else if i + 1 < rest.len() {
                args.insert("__args".into(), Value::String(rest[i + 1].clone()));
                i += 1;
            }
            i += 1;
            continue;
        }
        let prop = props.get(&key);
        let kind = prop
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("string");
        if kind == "boolean" {
            let value = match inline.as_deref() {
                Some("false") | Some("0") => false,
                Some(_) => true,
                None => true,
            };
            args.insert(key, Value::Bool(value));
            i += 1;
            continue;
        }
        let value_text = match inline {
            Some(v) => {
                i += 1;
                v
            }
            None => {
                if i + 1 >= rest.len() {
                    return Err(gry_core::GryError::invalid(format!(
                        "missing value for --{key}"
                    )));
                }
                i += 2;
                rest[i - 1].clone()
            }
        };
        let value =
            match kind {
                "integer" => value_text.parse::<i64>().map(|v| json!(v)).map_err(|_| {
                    gry_core::GryError::invalid(format!("invalid integer for --{key}"))
                })?,
                "number" => value_text.parse::<f64>().map(|v| json!(v)).map_err(|_| {
                    gry_core::GryError::invalid(format!("invalid number for --{key}"))
                })?,
                "array" => {
                    let parsed: Value = serde_json::from_str(&value_text)
                        .unwrap_or_else(|_| Value::String(value_text.clone()));
                    if parsed.is_array() {
                        parsed
                    } else {
                        match args.get_mut(&key) {
                            Some(Value::Array(items)) => {
                                items.push(parsed);
                                continue;
                            }
                            _ => Value::Array(vec![parsed]),
                        }
                    }
                }
                "object" => serde_json::from_str(&value_text).map_err(|e| {
                    gry_core::GryError::invalid(format!("invalid JSON object for --{key}: {e}"))
                })?,
                _ => Value::String(value_text),
            };
        args.insert(key, value);
    }
    Ok(args)
}

fn test_impact_cmd(config: &Config, cmd: &TestImpactCmd) -> gry_core::GryResult<()> {
    match &cmd.command {
        TestImpactSub::Select(req) => {
            ensure_indexed(config, &req.repo)?;
            let store = open_store(config)?;
            let value = select_tests(
                &store,
                &SelectRequest {
                    repo: req.repo.clone(),
                    base: req.base.clone(),
                    since: req.since.clone(),
                    config_path: req.config.clone(),
                    coverage_dir: req.coverage.clone(),
                    platform: req.platform.clone(),
                },
            )?;
            print_value(&value);
            Ok(())
        }
        TestImpactSub::Publish(req) => {
            ensure_indexed(config, &req.repo)?;
            let mut store = open_store(config)?;
            let value = publish(
                &mut store,
                &PublishRequest {
                    repo: req.repo.clone(),
                    out: req.out.clone(),
                    coverage_dir: req.coverage.clone(),
                    platform: req.platform.clone(),
                },
            )?;
            print_value(&value);
            Ok(())
        }
    }
}

fn ensure_indexed(config: &Config, repo: &str) -> gry_core::GryResult<()> {
    let root = config.resolve_index_root(repo)?;
    let store = open_store(config)?;
    if store.project_by_root(&root.to_string_lossy())?.is_some() {
        return Ok(());
    }
    drop(store);
    let indexer = Indexer::new(config.clone());
    indexer.index(&IndexRequest::new(root.to_string_lossy()))?;
    Ok(())
}

fn config_cmd(config: &Config, cmd: &ConfigCmd) -> gry_core::GryResult<()> {
    let mut config = config.clone();
    match &cmd.command {
        ConfigSub::List => {
            let rows: Vec<Value> = all_settings(&config.settings)
                .into_iter()
                .map(|(k, v)| json!({"key": k, "value": v}))
                .collect();
            print_value(&json!({"settings": rows, "path": config.settings_path()}));
        }
        ConfigSub::Get { key } => {
            print_value(&json!({"key": key, "value": get_setting(&config.settings, key)?}));
        }
        ConfigSub::Set { key, value } => {
            let canonical = set_setting(&mut config.settings, key, value)?;
            config.save_settings()?;
            print_value(&json!({"key": key, "value": canonical, "status": "stored"}));
        }
        ConfigSub::Reset { key } => {
            let canonical = reset_setting(&mut config.settings, key)?;
            config.save_settings()?;
            print_value(&json!({"key": key, "value": canonical, "status": "reset"}));
        }
    }
    Ok(())
}

const SUPPORTED_CLIENTS: &[(&str, &str)] = &[
    ("claude", "Claude Desktop / Claude Code (~/.claude.json)"),
    ("cursor", "Cursor (~/.cursor/mcp.json)"),
    ("windsurf", "Windsurf (~/.codeium/windsurf/mcp_config.json)"),
    ("gemini", "Gemini CLI (~/.gemini/settings.json)"),
    ("opencode", "OpenCode (~/.config/opencode/opencode.json)"),
    ("codex", "Codex CLI (~/.codex/config.toml)"),
    ("zed", "Zed (~/.config/zed/settings.json)"),
    ("cline", "Cline (VS Code global storage)"),
    ("roo", "Roo Code (VS Code global storage)"),
    ("kilo", "Kilo Code (VS Code global storage)"),
    ("vscode", "VS Code (~/.config/Code/User/mcp.json)"),
    ("trae", "Trae (~/.config/Trae/User/mcp.json)"),
    ("qwen", "Qwen Code (~/.qwen/settings.json)"),
    ("augment", "Augment (~/.augment/settings.json)"),
    ("amp", "Amp (~/.config/amp/settings.json)"),
];

fn install_cmd(config: &Config, cmd: &InstallCmd) -> gry_core::GryResult<()> {
    config.ensure_dirs()?;
    if cmd.list_clients {
        for (id, description) in SUPPORTED_CLIENTS {
            println!("{id}\t{description}");
        }
        return Ok(());
    }
    if cmd.skill {
        skill_cmd(&SkillCmd { install: true })?;
    }
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy().to_string();
    let path = cmd
        .config
        .clone()
        .unwrap_or_else(|| client_config_path(&cmd.client));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let client = cmd.client.as_str();
    if client == "codex" {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let block = format!("\n[mcp_servers.gry]\ncommand = {exe:?}\nargs = [\"serve\"]\n");
        let updated = if existing.contains("[mcp_servers.gry]") {
            replace_toml_section(&existing, "mcp_servers.gry", block.trim_start())
        } else {
            format!("{existing}{block}")
        };
        if cmd.dry_run {
            print_value(
                &json!({"client": client, "path": path, "dry_run": true, "content": updated}),
            );
        } else {
            backup(&path)?;
            std::fs::write(&path, updated)?;
            print_value(&json!({"client": client, "path": path, "status": "installed"}));
        }
        return Ok(());
    }
    let existing = std::fs::read(&path).ok();
    let mut root: Value = match &existing {
        Some(bytes) if !bytes.is_empty() => serde_json::from_slice(bytes).map_err(|e| {
            gry_core::GryError::invalid(format!(
                "refusing to edit {}: not valid JSON ({e})",
                path.display()
            ))
        })?,
        _ => json!({}),
    };
    if !root.is_object() {
        return Err(gry_core::GryError::invalid(format!(
            "refusing to edit {}: top-level JSON is not an object",
            path.display()
        )));
    }
    let entry = json!({
        "command": exe,
        "args": ["serve"],
    });
    match client {
        "opencode" => {
            if !root["mcp"].is_object() {
                root["mcp"] = json!({});
            }
            root["mcp"]["gry"] = json!({
                "type": "local",
                "command": [exe, "serve"],
                "enabled": true
            });
        }
        "zed" => {
            if !root["context_servers"].is_object() {
                root["context_servers"] = json!({});
            }
            root["context_servers"]["gry"] = json!({
                "command": {"path": exe, "args": ["serve"]}
            });
        }
        "vscode" => {
            if !root["servers"].is_object() {
                root["servers"] = json!({});
            }
            root["servers"]["gry"] = json!({
                "type": "stdio",
                "command": exe,
                "args": ["serve"]
            });
        }
        _ => {
            if !root["mcpServers"].is_object() {
                root["mcpServers"] = json!({});
            }
            root["mcpServers"]["gry"] = entry;
        }
    }
    if cmd.dry_run {
        print_value(&json!({"client": client, "path": path, "dry_run": true, "result": root}));
        return Ok(());
    }
    backup(&path)?;
    write_json_atomic(&path, &root)?;
    print_value(&json!({"client": client, "path": path, "status": "installed"}));
    Ok(())
}

fn uninstall_cmd(cmd: &UninstallCmd) -> gry_core::GryResult<()> {
    let path = cmd
        .config
        .clone()
        .unwrap_or_else(|| client_config_path(&cmd.client));
    if !path.exists() {
        print_value(&json!({"client": cmd.client, "path": path, "status": "not_found"}));
        return Ok(());
    }
    if cmd.client == "codex" {
        let text = std::fs::read_to_string(&path)?;
        let updated = remove_toml_section(&text, "mcp_servers.gry");
        if cmd.dry_run {
            print_value(
                &json!({"client": cmd.client, "path": path, "dry_run": true, "content": updated}),
            );
        } else {
            std::fs::write(&path, updated)?;
            print_value(&json!({"client": cmd.client, "path": path, "status": "removed"}));
        }
        return Ok(());
    }
    let bytes = std::fs::read(&path)?;
    let mut root: Value = serde_json::from_slice(&bytes).map_err(|e| {
        gry_core::GryError::invalid(format!("cannot parse {}: {e}", path.display()))
    })?;
    match cmd.client.as_str() {
        "opencode" => {
            if let Some(map) = root.get_mut("mcp").and_then(Value::as_object_mut) {
                map.remove("gry");
            }
        }
        "zed" => {
            if let Some(map) = root
                .get_mut("context_servers")
                .and_then(Value::as_object_mut)
            {
                map.remove("gry");
            }
        }
        "vscode" => {
            if let Some(map) = root.get_mut("servers").and_then(Value::as_object_mut) {
                map.remove("gry");
            }
        }
        _ => {
            if let Some(map) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
                map.remove("gry");
            }
        }
    }
    if cmd.dry_run {
        print_value(&json!({"client": cmd.client, "path": path, "dry_run": true, "result": root}));
    } else {
        write_json_atomic(&path, &root)?;
        print_value(&json!({"client": cmd.client, "path": path, "status": "removed"}));
    }
    Ok(())
}

fn skill_cmd(cmd: &SkillCmd) -> gry_core::GryResult<()> {
    let skill = r#"---
name: gry
description: Use gry's knowledge graph for structural code questions: where a symbol is defined, who calls it, call chains, architecture, impact and coverage.
---

# gry code intelligence

gry is a local code knowledge graph served over MCP. Prefer it over file-by-file
exploration for structural questions.

- `search_graph` — find symbols by name, regex, BM25 or semantic query.
- `get_code_snippet` — read a symbol's exact source by qualified name.
- `trace_path` — callers, callees, data flow, cross-service and infrastructure paths.
- `query_graph` — read-only Cypher for multi-hop or aggregate questions.
- `get_architecture` — languages, packages, entry points, routes, hotspots, cycles.
- `search_code` — literal/regex text search with graph context.
- `detect_changes` — map a diff to affected symbols and tests.
- `check_index_coverage` — verify a cited file was actually indexed; clean is not proof of completeness.

Paginate with the returned offsets/cursors. Use `search_code` or grep for
literal text, configuration values, comments and non-code files. If a result is
ambiguous or unresolved, say so rather than guessing.
"#;
    if cmd.install {
        let dir = dirs_config().join("gry");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("SKILL.md"), skill)?;
        print_value(&json!({"status": "installed", "path": dir.join("SKILL.md")}));
    } else {
        println!("{skill}");
    }
    Ok(())
}

fn client_config_path(client: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match client {
        "claude" => home.join(".claude.json"),
        "cursor" => home.join(".cursor/mcp.json"),
        "windsurf" => home.join(".codeium/windsurf/mcp_config.json"),
        "gemini" => home.join(".gemini/settings.json"),
        "opencode" => home.join(".config/opencode/opencode.json"),
        "codex" => home.join(".codex/config.toml"),
        "zed" => home.join(".config/zed/settings.json"),
        "cline" => home.join(".config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"),
        "roo" => home.join(".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json"),
        "kilo" => home.join(".config/Code/User/globalStorage/kilocode.kilo-code/settings/mcp_settings.json"),
        "vscode" => home.join(".config/Code/User/mcp.json"),
        "trae" => home.join(".config/Trae/User/mcp.json"),
        "qwen" => home.join(".qwen/settings.json"),
        "augment" => home.join(".augment/settings.json"),
        "amp" => home.join(".config/amp/settings.json"),
        other => home.join(format!(".{other}/mcp.json")),
    }
}

fn dirs_config() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn backup(path: &Path) -> gry_core::GryResult<()> {
    if path.exists() {
        let bak = path.with_extension(format!(
            "{}gry-bak",
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| format!("{e}."))
                .unwrap_or_default()
        ));
        std::fs::copy(path, bak)?;
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &Value) -> gry_core::GryResult<()> {
    let tmp = path.with_extension("gry.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn replace_toml_section(text: &str, section: &str, replacement: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let is_target = trimmed.trim_matches(['[', ']']).trim() == section;
            if skipping && !is_target {
                skipping = false;
            }
            if is_target {
                skipping = true;
                out.push(replacement.to_string());
                continue;
            }
        }
        if !skipping {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn remove_toml_section(text: &str, section: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            skipping = trimmed.trim_matches(['[', ']']).trim() == section;
        }
        if !skipping {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}
