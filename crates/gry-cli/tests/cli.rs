use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_gry")
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fixture() -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(&root, "src/lib.rs", "pub struct Engine;\nimpl Engine { pub fn parse(&self) -> usize { 1 } }\nfn main() { let e = Engine; e.parse(); }\n");
    write(&root, "docs/guide.md", "# Guide\n\n`Engine`.\n");
    (dir, root)
}

fn run(cache: &Path, args: &[&str]) -> Value {
    let output = Command::new(bin())
        .arg("--cache-dir")
        .arg(cache)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gry {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&output.stdout).to_string()))
}

#[test]
fn index_search_status_config_and_delete() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    let index = run(
        &cache,
        &["index", root.to_str().unwrap(), "--name", "cli-fixture"],
    );
    assert!(index["nodes"].as_i64().unwrap() > 5);
    let search = run(
        &cache,
        &["search", "cli-fixture", "parse", "--label", "Method"],
    );
    assert!(search["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "parse"));
    let status = run(&cache, &["status", "cli-fixture"]);
    assert_eq!(status["indexed"], true);
    let projects = run(&cache, &["projects"]);
    assert!(projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "cli-fixture"));

    let list = run(&cache, &["config", "list"]);
    assert!(list["settings"].as_array().unwrap().len() >= 5);
    let set = run(&cache, &["config", "set", "auto_index", "true"]);
    assert_eq!(set["value"], "true");
    let get = run(&cache, &["config", "get", "auto_index"]);
    assert_eq!(get["value"], "true");
    let reset = run(&cache, &["config", "reset", "auto_index"]);
    assert_eq!(reset["value"], "false");
    let mem = run(&cache, &["config", "set", "index_max_memory_mb", "2048"]);
    assert_eq!(mem["value"], "2048");
    let mem = run(&cache, &["config", "get", "index_max_memory_mb"]);
    assert_eq!(mem["value"], "2048");
    run(&cache, &["config", "reset", "index_max_memory_mb"]);
    let doctor = run(&cache, &["doctor"]);
    assert_eq!(doctor["ok"], true, "doctor: {doctor}");

    let deleted = run(&cache, &["delete", "cli-fixture"]);
    assert_eq!(deleted["status"], "deleted");
}

#[test]
fn generic_tool_and_serve_stdio() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    run(
        &cache,
        &["index", root.to_str().unwrap(), "--name", "cli-serve"],
    );

    let cli = run(
        &cache,
        &[
            "cli",
            "search_graph",
            "--project",
            "cli-serve",
            "--label",
            "Method",
            "--format",
            "json",
        ],
    );
    assert!(cli["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "parse"));

    let mut child = Command::new(bin())
        .arg("--cache-dir")
        .arg(&cache)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}})).unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"cursor":"6"}})
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"cursor":"12"}})
        )
        .unwrap();
        writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_graph","arguments":{"project":"cli-serve","label":"Method"}}})).unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":6,"method":"shutdown"})
        )
        .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let messages: Vec<Value> = stdout
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(messages
        .iter()
        .any(|m| m["result"]["serverInfo"]["name"] == "gry"));
    let tool_pages: usize = messages
        .iter()
        .filter_map(|m| m["result"]["tools"].as_array().map(Vec::len))
        .sum();
    assert_eq!(tool_pages, 18);
    assert!(messages.iter().any(|m| m["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .contains("parse")));
}

#[test]
fn install_dry_run_is_non_destructive() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(bin())
        .env("HOME", home.path())
        .args(["install", "--client", "claude", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.path().join(".claude.json").exists());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["dry_run"], true);

    let output = Command::new(bin())
        .env("HOME", home.path())
        .args(["install", "--client", "claude"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: Value =
        serde_json::from_slice(&fs::read(home.path().join(".claude.json")).unwrap()).unwrap();
    assert_eq!(config["mcpServers"]["gry"]["args"][0], "serve");

    let output = Command::new(bin())
        .env("HOME", home.path())
        .args(["uninstall", "--client", "claude"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: Value =
        serde_json::from_slice(&fs::read(home.path().join(".claude.json")).unwrap()).unwrap();
    assert!(config["mcpServers"].get("gry").is_none());
}

#[test]
fn install_auto_detects_configured_harnesses() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".claude.json"), "{}").unwrap();
    fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
    fs::write(home.path().join(".config/opencode/opencode.json"), "{}").unwrap();
    fs::create_dir_all(home.path().join(".omp/agent")).unwrap();
    fs::write(
        home.path().join(".omp/agent/mcp.json"),
        "{\"mcpServers\":{}}",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["--json", "install", "--client", "auto"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "configured");
    let configured: Vec<&str> = value["configured"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["client"].as_str())
        .collect();
    assert!(configured.contains(&"claude"), "{value}");
    assert!(configured.contains(&"opencode"), "{value}");
    assert!(configured.contains(&"omp"), "{value}");

    let claude: Value =
        serde_json::from_slice(&fs::read(home.path().join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"]["gry"]["args"][0], "serve");
    let opencode: Value = serde_json::from_slice(
        &fs::read(home.path().join(".config/opencode/opencode.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(opencode["mcp"]["gry"]["command"][1], "serve");
    let omp: Value =
        serde_json::from_slice(&fs::read(home.path().join(".omp/agent/mcp.json")).unwrap())
            .unwrap();
    assert_eq!(omp["mcpServers"]["gry"]["type"], "stdio");
    assert_eq!(omp["mcpServers"]["gry"]["args"][0], "serve");
    assert!(!home.path().join(".codex/config.toml").exists());

    let output = Command::new(bin())
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["--json", "uninstall", "--client", "auto"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let claude: Value =
        serde_json::from_slice(&fs::read(home.path().join(".claude.json")).unwrap()).unwrap();
    assert!(claude["mcpServers"].get("gry").is_none());
    let opencode: Value = serde_json::from_slice(
        &fs::read(home.path().join(".config/opencode/opencode.json")).unwrap(),
    )
    .unwrap();
    assert!(opencode["mcp"].get("gry").is_none());
    let omp: Value =
        serde_json::from_slice(&fs::read(home.path().join(".omp/agent/mcp.json")).unwrap())
            .unwrap();
    assert!(omp["mcpServers"].get("gry").is_none());
}

#[test]
fn install_skill_targets_detected_harnesses() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".claude.json"), "{}").unwrap();
    fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
    fs::write(home.path().join(".config/opencode/opencode.json"), "{}").unwrap();
    fs::create_dir_all(home.path().join(".omp/agent")).unwrap();
    fs::write(
        home.path().join(".omp/agent/mcp.json"),
        "{\"mcpServers\":{}}",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["--quiet", "install", "--client", "claude", "--skill"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.path().join(".claude/skills/gry/SKILL.md").exists());
    assert!(home
        .path()
        .join(".config/opencode/skills/gry/SKILL.md")
        .exists());
    assert!(home.path().join(".omp/agent/skills/gry/SKILL.md").exists());
    assert!(home.path().join(".config/gry/SKILL.md").exists());
}

#[test]
fn install_all_configures_every_client() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(bin())
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["--json", "install", "--client", "all"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "configured");
    assert_eq!(value["configured"].as_array().unwrap().len(), 18);
    assert_eq!(value["failed"].as_array().unwrap().len(), 0);
    assert!(home.path().join(".codex/config.toml").exists());
    assert!(home.path().join(".cursor/mcp.json").exists());
    assert!(home.path().join(".omp/agent/mcp.json").exists());
    if cfg!(target_os = "macos") {
        assert!(home
            .path()
            .join("Library/Application Support/Code/User/mcp.json")
            .exists());
        assert!(home
            .path()
            .join("Library/Application Support/Claude/claude_desktop_config.json")
            .exists());
    } else {
        assert!(home.path().join(".config/Code/User/mcp.json").exists());
        assert!(home
            .path()
            .join(".config/Claude/claude_desktop_config.json")
            .exists());
    }
}

#[test]
fn ohgo_install_preserves_settings_and_classifies_tools() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join(".ohgo/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"model":"groq/openai/gpt-oss-20b","identity":{"name":"dev"}}"#,
    )
    .unwrap();

    let output = Command::new(bin())
        .env("HOME", home.path())
        .args(["install", "--client", "ohgo"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(config["model"], "groq/openai/gpt-oss-20b");
    assert_eq!(config["identity"]["name"], "dev");
    let servers = config["mcp"]["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "gry");
    assert_eq!(servers[0]["args"][0], "serve");
    assert_eq!(servers[0]["env"][0], "GRY_CACHE_DIR");
    assert_eq!(servers[0]["tools"]["search_graph"], "read");
    assert_eq!(servers[0]["tools"]["index_repository"], "mutate");
    assert!(servers[0]["allowMutation"].is_null());

    // Re-installing replaces the gry entry instead of duplicating it.
    let output = Command::new(bin())
        .env("HOME", home.path())
        .args(["install", "--client", "ohgo"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(config["mcp"]["servers"].as_array().unwrap().len(), 1);

    let output = Command::new(bin())
        .env("HOME", home.path())
        .args(["uninstall", "--client", "ohgo"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(config["model"], "groq/openai/gpt-oss-20b");
    assert!(config["mcp"]["servers"].as_array().unwrap().is_empty());
}

#[test]
fn memory_budget_rejects_clearly_oversized_estimates() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    let output = Command::new(bin())
        .arg("--cache-dir")
        .arg(&cache)
        .env("GRY_MAX_INDEX_MEMORY_MB", "1")
        .args(["index", root.to_str().unwrap(), "--name", "budget"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("memory budget"));
}

#[test]
fn global_json_is_compact_on_one_line() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    run(
        &cache,
        &["index", root.to_str().unwrap(), "--name", "json-mode"],
    );
    let output = Command::new(bin())
        .arg("--cache-dir")
        .arg(&cache)
        .args(["--json", "projects"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.lines().count(), 1, "expected compact JSON: {text}");
    assert!(serde_json::from_str::<Value>(text.trim()).is_ok());
}

#[test]
fn async_index_and_status() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    let queued = run(
        &cache,
        &[
            "index",
            root.to_str().unwrap(),
            "--name",
            "cli-async",
            "--async",
        ],
    );
    assert_eq!(queued["state"], "queued");
    let mut succeeded = false;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let status = run(
            &cache,
            &[
                "index",
                root.to_str().unwrap(),
                "--name",
                "cli-async",
                "--status",
            ],
        );
        let state = status["state"].as_str().unwrap_or("");
        if state == "succeeded" {
            succeeded = true;
            break;
        }
        if state == "failed" {
            panic!("async index failed: {status}");
        }
    }
    assert!(succeeded, "async index did not finish");
}

#[test]
fn test_impact_select_and_publish() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(
        &root,
        "src/math.c",
        "int add(int a, int b) { return a + b; }\nint mul(int a, int b) { return add(a,b) * a; }\n",
    );
    write(
        &root,
        "tests/test_math.c",
        "int add(int a, int b);\nint main(void) { return add(1,2) == 3 ? 0 : 1; }\n",
    );
    write(
        &root,
        ".gry.json",
        r#"{"test_impact":{"version":1,"tests":{"presets":{"c":true},"test_globs":["tests/**"]},"rules":[{"id":"src","paths":["src/**"],"action":"referencing_tests"},{"id":"tests","paths":["tests/**"],"action":"referencing_tests"}],"unmapped_files":"run_all"}}"#,
    );
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "a@b.c"]);
    git(&["config", "user.name", "A"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    let commit = {
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let cache = dir.path().join("cache");
    run(
        &cache,
        &["index", root.to_str().unwrap(), "--name", "impact"],
    );
    let coverage = dir.path().join("coverage");
    fs::create_dir_all(&coverage).unwrap();
    fs::write(coverage.join("meta.json"), serde_json::to_vec(&json!({"commit":commit,"platform":"test-platform","created_at":"2026-10-25T00:00:00Z"})).unwrap()).unwrap();
    fs::write(
        coverage.join("tests.tsv"),
        "impact.src.math.add\timpact.tests.test_math.main\n",
    )
    .unwrap();
    write(&root, "src/math.c", "int add(int a, int b) { return a + b + 0; }\nint mul(int a, int b) { return add(a,b) * a; }\n");
    let selection = run(
        &cache,
        &[
            "test-impact",
            "select",
            "--repo",
            root.to_str().unwrap(),
            "--base",
            "main",
            "--coverage",
            coverage.to_str().unwrap(),
            "--platform",
            "test-platform",
        ],
    );
    assert_eq!(selection["run_all"], false, "selection: {selection}");
    assert!(selection["selected_tests"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "main"));
    let out = dir.path().join("bundle");
    let receipt = run(
        &cache,
        &[
            "test-impact",
            "publish",
            "--repo",
            root.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--coverage",
            coverage.to_str().unwrap(),
            "--platform",
            "test-platform",
        ],
    );
    assert!(out.join("graph.db.zst").is_file());
    assert!(out.join("receipt.json").is_file());
    assert_eq!(receipt["commit"], commit);
}

#[test]
fn context_and_lifecycle_hooks_are_bounded_and_explicit() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    run(
        &cache,
        &["index", root.to_str().unwrap(), "--name", "cli-context"],
    );

    let context = run(
        &cache,
        &[
            "context",
            "cli-context",
            "--target",
            "parse",
            "--profile",
            "scout",
            "--max-tokens",
            "800",
        ],
    );
    assert_eq!(context["status"], "ok");
    assert_eq!(context["profile"], "scout");
    assert!(context["budget"]["estimated_tokens"].as_u64().unwrap() <= 800);
    assert!(context["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["name"] == "parse"));

    let first = run(
        &cache,
        &[
            "hook",
            "before_edit",
            "--project",
            "cli-context",
            "--target",
            "parse",
            "--max-tokens",
            "800",
        ],
    );
    assert_eq!(first["hook"]["status"], "ok");
    assert_eq!(first["hook"]["cached"], false);
    let second = run(
        &cache,
        &[
            "hook",
            "before_edit",
            "--project",
            "cli-context",
            "--target",
            "parse",
            "--max-tokens",
            "800",
        ],
    );
    assert_eq!(second["hook"]["cached"], true);

    let missing = run(
        &cache,
        &[
            "hook",
            "before_plan",
            "--project",
            "not-indexed",
            "--target",
            "x",
        ],
    );
    assert_eq!(missing["hook"]["status"], "not_indexed");
    assert_eq!(missing["failure"]["code"], "not_indexed");
    assert!(missing["context"].is_null());

    let recursive = Command::new(bin())
        .arg("--cache-dir")
        .arg(&cache)
        .env("GRY_HOOK_ACTIVE", "1")
        .args([
            "hook",
            "before_edit",
            "--project",
            "cli-context",
            "--target",
            "parse",
        ])
        .output()
        .unwrap();
    assert!(recursive.status.success());
    let recursive: Value = serde_json::from_slice(&recursive.stdout).unwrap();
    assert_eq!(recursive["hook"]["status"], "unavailable");
    assert_eq!(recursive["failure"]["code"], "recursive_hook");
}

#[test]
fn context_unknown_project_fails_with_a_typed_error() {
    let (dir, root) = fixture();
    let cache = dir.path().join("cache");
    run(
        &cache,
        &[
            "index",
            root.to_str().unwrap(),
            "--name",
            "cli-context-error",
        ],
    );
    let output = Command::new(bin())
        .arg("--cache-dir")
        .arg(&cache)
        .args(["context", "missing-project", "--target", "parse"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("project not found or not indexed"));
}
