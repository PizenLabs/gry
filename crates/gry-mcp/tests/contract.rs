use gry_core::config::{Config, RuntimeSettings};
use gry_mcp::{Server, ToolProfile};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(
        &root,
        "src/lib.rs",
        r#"
use crate::parser::Parser;
pub struct Engine { count: usize }
impl Engine {
    /// Parse a request.
    pub fn parse(&mut self, input: &str) -> usize {
        let p = Parser::new();
        p.run(input)
    }
}
fn main() { let mut e = Engine { count: 0 }; e.parse("x"); }
"#,
    );
    write(
        &root,
        "src/parser.rs",
        "pub struct Parser;\nimpl Parser {\n pub fn new() -> Self { Parser }\n pub fn run(&self, input: &str) -> usize { input.len() }\n}\n",
    );
    write(
        &root,
        "tests/test_engine.rs",
        "#[test]\nfn test_parse() { let mut e = crate::Engine { count: 0 }; assert_eq!(e.parse(\"abc\"), 3); }\n",
    );
    write(
        &root,
        "Dockerfile",
        "FROM alpine:3.20\nENV PORT=8080\nENTRYPOINT [\"/app/server\"]\n",
    );
    write(
        &root,
        "docs/guide.md",
        "# Guide\n\nThe `Engine` parses requests. See [parser](src/parser.rs).\n",
    );
    write(&root, "docs/adr/0001-storage.md", "# ADR-1 Storage\n\nStatus: Accepted\nDate: 2026-01-01\n\n## Context\n\nNeed storage.\n\n## Decision\n\nUse SQLite.\n");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "a@b.c"]);
    git(&root, &["config", "user.name", "A"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    dir
}

fn server(dir: &TempDir) -> Server {
    let config = Config {
        cache_dir: dir.path().join("cache"),
        db_path: dir.path().join("cache/graph.db"),
        allowed_root: None,
        workers: 2,
        max_files: None,
        max_source_mb: None,
        settings: RuntimeSettings::default(),
    };
    Server::new(config, ToolProfile::All)
}

fn call(server: &mut Server, name: &str, args: Value) -> Value {
    let response = server
        .handle_message(json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":name,"arguments":args}
        }))
        .expect("response");
    assert!(
        response.get("error").is_none(),
        "error for {name}: {response}"
    );
    let result = &response["result"];
    assert_eq!(result["isError"], false, "tool error for {name}: {result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

#[test]
fn all_seventeen_tools_succeed() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let mut srv = server(&dir);
    let root_str = root.to_string_lossy().to_string();

    let index = call(&mut srv, "index_repository", json!({"repo_path": root_str}));
    assert_eq!(index["project"], "repo");
    assert!(index["nodes"].as_i64().unwrap() > 20);

    let projects = call(&mut srv, "list_projects", json!({"detail":"stats"}));
    assert_eq!(projects["projects"][0]["name"], "repo");
    let status = call(
        &mut srv,
        "index_status",
        json!({"project":"repo","diagnostics":"summary"}),
    );
    assert_eq!(status["indexed"], true);
    let coverage = call(
        &mut srv,
        "check_index_coverage",
        json!({"project":"repo","paths":["src/lib.rs","missing.rs"],"scopes":["src"]}),
    );
    assert_eq!(coverage["paths"][0]["status"], "indexed");
    assert_eq!(coverage["paths"][1]["status"], "missing");
    let search = call(
        &mut srv,
        "search_graph",
        json!({"project":"repo","query":"parse","label":"Method"}),
    );
    assert!(search["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "parse"));
    let query = call(
        &mut srv,
        "query_graph",
        json!({"project":"repo","query":"MATCH (a)-[:CALLS]->(b) RETURN a.name,b.name"}),
    );
    assert!(query["rows"].as_array().unwrap().len() >= 1);
    let trace = call(
        &mut srv,
        "trace_path",
        json!({"project":"repo","function_name":"repo.src.lib.Engine.parse","direction":"outbound"}),
    );
    assert!(trace["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "new"));
    let snippet = call(
        &mut srv,
        "get_code_snippet",
        json!({"project":"repo","qualified_name":"repo.src.lib.Engine","source_mode":"outline"}),
    );
    assert_eq!(snippet["source_mode"], "outline");
    let outline = call(
        &mut srv,
        "get_file_outline",
        json!({"project":"repo","file_path":"src/lib.rs"}),
    );
    assert!(outline["total"].as_i64().unwrap() >= 3);
    let schema = call(
        &mut srv,
        "get_graph_schema",
        json!({"project":"repo","diagnostics":"full"}),
    );
    assert!(schema["labels"].as_array().unwrap().len() > 5);
    let compare = call(
        &mut srv,
        "compare_graphs",
        json!({"base_project":"repo","target_project":"repo"}),
    );
    assert_eq!(compare["nodes_added"]["total"], 0);
    let arch = call(
        &mut srv,
        "get_architecture",
        json!({"project":"repo","aspects":["overview"]}),
    );
    assert!(arch["languages"].as_array().unwrap().len() >= 2);
    let code = call(
        &mut srv,
        "search_code",
        json!({"project":"repo","pattern":"Parser","mode":"compact"}),
    );
    assert!(code["results"].as_array().unwrap().len() >= 1);

    // Worktree change for detect_changes.
    write(&root, "src/parser.rs", "pub struct Parser;\nimpl Parser {\n pub fn new() -> Self { Parser }\n pub fn run(&self, input: &str) -> usize { input.len() + 1 }\n}\n");
    let changes = call(
        &mut srv,
        "detect_changes",
        json!({"project":"repo","scope":"tests","base_branch":"main"}),
    );
    assert!(changes["changed_files"]["total"].as_i64().unwrap() >= 1);

    let adr = call(
        &mut srv,
        "manage_adr",
        json!({"project":"repo","mode":"update","content":"# ADR\n\n## DECISION\n\nUse SQLite.\n"}),
    );
    assert_eq!(adr["status"], "stored");
    let adr = call(
        &mut srv,
        "manage_adr",
        json!({"project":"repo","mode":"set_sections","section_updates":{"DECISION":"Use SQLite and WAL."}}),
    );
    assert_eq!(adr["status"], "stored");
    let adr = call(
        &mut srv,
        "manage_adr",
        json!({"project":"repo","mode":"get"}),
    );
    assert!(adr["content"].as_str().unwrap().contains("WAL"));

    let traces = call(
        &mut srv,
        "ingest_traces",
        json!({"project":"repo","label":"test","traces":[{"path":["repo.src.lib.main","repo.src.lib.Engine.parse"],"count":3}]}),
    );
    assert_eq!(traces["stored_calls"], 1);

    let deleted = call(&mut srv, "delete_project", json!({"project":"repo"}));
    assert_eq!(deleted["status"], "deleted");
}

#[test]
fn errors_and_prompts() {
    let dir = fixture();
    let mut srv = server(&dir);
    let unknown = srv
        .handle_message(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope","arguments":{}}}))
        .unwrap();
    assert_eq!(unknown["error"]["code"], -32602);
    let missing = srv
        .handle_message(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_graph","arguments":{}}}))
        .unwrap();
    assert_eq!(missing["result"]["isError"], true);
    let prompt = srv
        .handle_message(json!({"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"review_change_impact","arguments":{"project":"p","change":"x"}}}))
        .unwrap();
    assert!(prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .contains("detect_changes"));
}
