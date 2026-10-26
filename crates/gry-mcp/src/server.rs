//! MCP JSON-RPC 2.0 server over stdio.

use crate::handlers::{dispatch, Ctx};
use crate::render::render;
use crate::tools::{instructions, tool_json, tools, ToolProfile};
use gry_core::config::Config;
use gry_index::jobs::JobManager;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};

const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const TOOLS_PAGE_SIZE: usize = 6;

pub struct Server {
    pub config: Config,
    pub profile: ToolProfile,
    pub jobs: JobManager,
    pub last_project: Mutex<Option<String>>,
    pub roots: Mutex<Vec<String>>,
    initialized: bool,
    shutdown: bool,
}

impl Server {
    pub fn new(config: Config, profile: ToolProfile) -> Self {
        Self {
            config,
            profile,
            jobs: JobManager::new(),
            last_project: Mutex::new(None),
            roots: Mutex::new(Vec::new()),
            initialized: false,
            shutdown: false,
        }
    }

    pub fn serve_stdio(&mut self) -> gry_core::GryResult<()> {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        loop {
            let Some(message) = read_message(&mut reader)? else {
                break;
            };
            let response = if message.is_array() {
                let mut responses = Vec::new();
                for item in message.as_array().cloned().unwrap_or_default() {
                    if let Some(resp) = self.handle(item) {
                        responses.push(resp);
                    }
                }
                if responses.is_empty() {
                    None
                } else {
                    Some(Value::Array(responses))
                }
            } else {
                self.handle(message)
            };
            if let Some(response) = response {
                let text = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                writer.write_all(text.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
            if self.shutdown {
                break;
            }
        }
        Ok(())
    }

    /// Handles one JSON-RPC message. Public for contract tests and embedders.
    pub fn handle_message(&mut self, message: Value) -> Option<Value> {
        self.handle(message)
    }

    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = match message.get("method").and_then(Value::as_str) {
            Some(method) => method,
            None => {
                return Some(error_response(
                    id,
                    -32600,
                    "invalid request: missing method",
                ))
            }
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let is_notification = id.is_none();
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "notifications/initialized" => {
                self.initialized = true;
                Ok(Value::Null)
            }
            "notifications/cancelled" => Ok(Value::Null),
            "ping" => Ok(json!({})),
            "shutdown" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            "exit" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            "tools/list" => self.tools_list(&params),
            "tools/call" => self.tools_call(&params),
            "prompts/list" => Ok(prompts_list()),
            "prompts/get" => self.prompts_get(&params),
            "roots/list" => Ok(json!({"roots": self.roots.lock().clone()})),
            "logging/setLevel" => Ok(json!({})),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        if is_notification {
            return None;
        }
        match result {
            Ok(value) => Some(json!({"jsonrpc": "2.0", "id": id, "result": value})),
            Err((code, message)) => Some(error_response(id, code, &message)),
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        let version = if PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            PROTOCOL_VERSIONS[0]
        };
        if let Some(roots) = params.get("roots").and_then(Value::as_array) {
            let mut collected = Vec::new();
            for root in roots {
                if let Some(uri) = root.get("uri").and_then(Value::as_str) {
                    if let Some(path) = uri.strip_prefix("file://") {
                        collected.push(percent_decode(path));
                    } else if uri.starts_with('/') {
                        collected.push(uri.to_string());
                    }
                }
                if let Some(path) = root.get("path").and_then(Value::as_str) {
                    collected.push(path.to_string());
                }
            }
            if self.config.settings.auto_index {
                for root in &collected {
                    let mut request = gry_index::pipeline::IndexRequest::new(root.clone());
                    request.mode = "full".to_string();
                    let _ = self.jobs.start(self.config.clone(), request);
                }
            }
            *self.roots.lock() = collected;
        }
        let profile = self.profile.name();
        json!({
            "protocolVersion": version,
            "serverInfo": {
                "name": "gry",
                "version": env!("CARGO_PKG_VERSION"),
                "title": "gry code intelligence"
            },
            "capabilities": {
                "tools": {"listChanged": false},
                "prompts": {"listChanged": false}
            },
            "instructions": instructions(self.profile),
            "_meta": {"toolProfile": profile}
        })
    }

    fn tools_list(&self, params: &Value) -> Result<Value, (i64, String)> {
        let all = tools();
        let visible: Vec<_> = all
            .into_iter()
            .filter(|tool| crate::tools::tool_visible(self.profile, tool))
            .collect();
        let offset = match params.get("cursor").and_then(Value::as_str) {
            None => 0,
            Some(cursor) => cursor
                .parse()
                .map_err(|_| (-32602, format!("invalid cursor: {cursor}")))?,
        };
        // The first page is bounded too. Returning every tool on the first
        // call and paginating only later cursors makes the advertised page
        // size depend on how the client happened to start, and forces large
        // tool lists into one response for spec-compliant clients.
        let offset = offset.min(visible.len());
        let end = (offset + TOOLS_PAGE_SIZE).min(visible.len());
        let page: Vec<_> = visible[offset..end].iter().collect();
        let next = if end < visible.len() {
            Some(end.to_string())
        } else {
            None
        };
        let mut out = json!({"tools": page.iter().map(|t| tool_json(t)).collect::<Vec<_>>()});
        if let Some(next) = next {
            out["nextCursor"] = json!(next);
        }
        Ok(out)
    }

    fn tools_call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| (-32602, "invalid params: missing tool name".to_string()))?;
        if !crate::tools::tools()
            .iter()
            .any(|tool| tool.name == name && crate::tools::tool_visible(self.profile, tool))
        {
            return Err((-32602, format!("unknown or unavailable tool: {name}")));
        }
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let args: Map<String, Value> = match args {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            _ => return Err((-32602, "arguments must be an object".to_string())),
        };
        let roots_guard = self.roots.lock();
        let ctx = Ctx {
            config: &self.config,
            jobs: &self.jobs,
            last_project: &self.last_project,
            roots: &roots_guard,
            profile: self.profile,
        };
        match dispatch(&ctx, name, &args) {
            Ok((value, format, max_tokens)) => {
                let text = render(
                    &value,
                    &format,
                    if max_tokens == 0 { 3200 } else { max_tokens },
                );
                Ok(json!({
                    "content": [{"type": "text", "text": text}],
                    "isError": false,
                }))
            }
            Err(error) => Ok(json!({
                "content": [{"type": "text", "text": error.to_string()}],
                "isError": true,
            })),
        }
    }

    fn prompts_get(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| (-32602, "invalid prompt name".to_string()))?;
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
        let project = arguments
            .get("project")
            .and_then(Value::as_str)
            .unwrap_or("");
        let request = if name == "explore_codebase" {
            arguments
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or("")
        } else if name == "review_change_impact" {
            arguments
                .get("change")
                .and_then(Value::as_str)
                .unwrap_or("")
        } else {
            return Err((-32602, "invalid prompt name".to_string()));
        };
        if project.is_empty() || request.is_empty() {
            return Err((-32602, "missing required prompt arguments".to_string()));
        }
        let (description, text) = if name == "explore_codebase" {
            (
                "Graph-first codebase exploration",
                format!(
                    "Explore project \"{project}\" to answer: {request}\n\nUse graph tools first: search_graph to find relevant symbols, get_code_snippet for exact source, and trace_path(direction=\"both\") for callers and callees. Use get_architecture for broad orientation and query_graph only for multi-hop patterns. Check has_more and paginate. Fall back to search_code or grep only for literal or non-code text, or where graph coverage is incomplete."
                ),
            )
        } else {
            let base = arguments
                .get("base_branch")
                .and_then(Value::as_str)
                .unwrap_or("main");
            (
                "Graph-first change-impact review",
                format!(
                    "Review change impact in project \"{project}\" for: {request}\n\nUse detect_changes with base_branch \"{base}\", then trace_path(direction=\"both\", include_tests=true) for affected callers, callees, and tests. Read exact definitions with get_code_snippet and use query_graph for cross-boundary patterns. Report affected callers, tests, boundaries, and risks; do not modify files."
                ),
            )
        };
        Ok(json!({
            "description": description,
            "messages": [{"role": "user", "content": {"type": "text", "text": text}}]
        }))
    }
}

fn prompts_list() -> Value {
    json!({
        "prompts": [
            {
                "name": "explore_codebase",
                "title": "Explore codebase",
                "description": "Explore a codebase with graph-first structural discovery.",
                "arguments": [
                    {"name":"project","title":"Project","description":"Indexed project name from list_projects.","required":true},
                    {"name":"question","title":"Question","description":"Architecture or implementation question to investigate.","required":true}
                ]
            },
            {
                "name": "review_change_impact",
                "title": "Review change impact",
                "description": "Review affected callers, tests, boundaries, and risks.",
                "arguments": [
                    {"name":"project","title":"Project","description":"Indexed project name from list_projects.","required":true},
                    {"name":"change","title":"Change","description":"Change, symbol, or area whose impact should be reviewed.","required":true},
                    {"name":"base_branch","title":"Base branch","description":"Git branch or ref for detect_changes; defaults to main.","required":false}
                ]
            }
        ]
    })
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {"code": code, "message": message}
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn read_message(reader: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        if line.trim().is_empty() {
            continue;
        }
        break;
    }
    let trimmed = line.trim_start();
    if trimmed.to_ascii_lowercase().starts_with("content-length:") {
        let length: usize = trimmed
            .split(':')
            .nth(1)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        // Consume remaining headers.
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 {
                return Ok(None);
            }
            if header.trim().is_empty() {
                break;
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
        return Ok(Some(serde_json::from_slice(&body).unwrap_or(Value::Null)));
    }
    Ok(Some(
        serde_json::from_str(line.trim()).unwrap_or_else(|_| Value::Null),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_tools(server: &mut Server) -> Vec<Value> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let response = server
                .handle(json!({"jsonrpc":"2.0","id":9,"method":"tools/list","params":params}))
                .unwrap();
            tools.extend(
                response["result"]["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            match response["result"]["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_string()),
                None => return tools,
            }
        }
    }

    #[test]
    fn tools_list_pages_from_the_first_page() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: None,
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: gry_core::config::RuntimeSettings::default(),
        };
        let mut server = Server::new(config, ToolProfile::All);
        let first = server
            .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .unwrap();
        assert_eq!(first["result"]["tools"].as_array().unwrap().len(), 6);
        assert_eq!(first["result"]["nextCursor"], "6");
        let all = all_tools(&mut server);
        assert_eq!(all.len(), 18);
        let names: Vec<&str> = all.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"query_graph"));
        assert!(names.contains(&"ingest_traces"));
    }

    #[test]
    fn initialize_negotiates_and_advertises_tools() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: None,
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: gry_core::config::RuntimeSettings::default(),
        };
        let mut server = Server::new(config, ToolProfile::All);
        let response = server
            .handle(json!({
                "jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2025-11-25"}
            }))
            .unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(response["result"]["serverInfo"]["name"], "gry");
        assert!(response["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("search_graph"));
        let tools = all_tools(&mut server);
        assert_eq!(tools.len(), 18);
        assert!(tools.iter().any(|t| t["name"] == "query_graph"));
    }

    #[test]
    fn analysis_profile_hides_writes() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: None,
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: gry_core::config::RuntimeSettings::default(),
        };
        let mut server = Server::new(config, ToolProfile::Analysis);
        let tools = all_tools(&mut server);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(!names.contains(&"index_repository"));
        assert!(!names.contains(&"delete_project"));
        assert!(names.contains(&"search_graph"));
    }

    #[test]
    fn scout_profile_exposes_task_context() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: None,
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: gry_core::config::RuntimeSettings::default(),
        };
        let mut server = Server::new(config, ToolProfile::Scout);
        let tools = all_tools(&mut server);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"get_task_context"));
        assert!(!names.contains(&"detect_changes"));
    }

    #[test]
    fn prompts_are_present_and_validated() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: None,
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: gry_core::config::RuntimeSettings::default(),
        };
        let mut server = Server::new(config, ToolProfile::All);
        let list = server
            .handle(json!({"jsonrpc":"2.0","id":3,"method":"prompts/list"}))
            .unwrap();
        assert_eq!(list["result"]["prompts"].as_array().unwrap().len(), 2);
        let get = server
            .handle(json!({
                "jsonrpc":"2.0","id":4,"method":"prompts/get",
                "params":{"name":"explore_codebase","arguments":{"project":"p","question":"q"}}
            }))
            .unwrap();
        assert!(get["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("search_graph"));
    }
}
