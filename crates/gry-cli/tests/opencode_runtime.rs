//! OpenCode adapter verification.
//!
//! The contract suite runs the generated plugin against a fake gry binary.
//! The live test is opt-in because it starts a real OpenCode background
//! service:
//!
//! ```sh
//! GRY_OPENCODE_LIVE=1 cargo test -p gry --test opencode_runtime -- --nocapture
//! ```

use serde_json::json;
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;

fn node() -> Option<String> {
    ["node", "nodejs"]
        .into_iter()
        .find(|name| Command::new(name).arg("--version").output().is_ok())
        .map(str::to_string)
}

#[test]
fn plugin_contract_suite_passes() {
    let Some(node) = node() else {
        eprintln!("skipping: node is not installed");
        return;
    };
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/opencode_plugin_test.mjs");
    let output = Command::new(node)
        .arg("--test")
        .arg(suite)
        .output()
        .expect("run node test suite");
    assert!(
        output.status.success(),
        "plugin contract suite failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn plugin_loads_in_the_real_runtime() {
    if std::env::var("GRY_OPENCODE_LIVE").as_deref() != Ok("1") {
        eprintln!("skipping: set GRY_OPENCODE_LIVE=1 to start a real OpenCode service");
        return;
    }
    let Some(opencode) = which("opencode") else {
        eprintln!("skipping: opencode is not installed");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let config = home.join(".config");
    let data = root.path().join("data");
    let state = root.path().join("state");
    let project = root.path().join("project");
    for dir in [&config, &data, &state, &project] {
        fs::create_dir_all(dir).unwrap();
    }
    let plugin_dir = config.join("opencode/plugins");
    fs::create_dir_all(&plugin_dir).unwrap();
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/opencode-gry.js"))
            .unwrap();
    let rendered = source.replace(
        "\"__GRY_BIN__\"",
        &serde_json::to_string("/bin/true").unwrap(),
    );
    fs::write(plugin_dir.join("gry.js"), rendered).unwrap();
    fs::write(
        config.join("opencode/opencode.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": {"servers": {"gry": {"type": "local", "command": ["/bin/true", "serve"]}}}
        }))
        .unwrap(),
    )
    .unwrap();

    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let run = |args: &[&str]| {
        Command::new(&opencode)
            .args(args)
            .current_dir(&project)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", &state)
            .output()
            .unwrap_or_else(|error| panic!("opencode {:?}: {error}", args))
    };
    let _ = run(&["service", "set", "port", &port.to_string()]);
    let output = run(&["debug", "agents"]);
    let _ = run(&["service", "stop"]);
    assert!(
        output.status.success(),
        "opencode debug agents failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(data.join("opencode/log/opencode.log")).unwrap_or_default();
    assert!(
        log.contains("plugins/gry.js"),
        "OpenCode did not discover the plugin:\n{log}"
    );
    assert!(
        !log.contains("failed to load plugin"),
        "OpenCode rejected the plugin:\n{log}"
    );
}

fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
}

#[test]
fn mcp_config_connects_in_the_real_runtime() {
    if std::env::var("GRY_OPENCODE_LIVE").as_deref() != Ok("1") {
        eprintln!("skipping: set GRY_OPENCODE_LIVE=1 to start a real OpenCode service");
        return;
    }
    let Some(opencode) = which("opencode") else {
        eprintln!("skipping: opencode is not installed");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let config = home.join(".config");
    let data = root.path().join("data");
    let state = root.path().join("state");
    let project = root.path().join("project");
    for dir in [&config, &data, &state, &project] {
        fs::create_dir_all(dir).unwrap();
    }
    let gry = Path::new(env!("CARGO_BIN_EXE_gry"))
        .to_string_lossy()
        .to_string();
    fs::create_dir_all(config.join("opencode")).unwrap();
    fs::write(
        config.join("opencode/opencode.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": {"servers": {"gry": {"type": "local", "command": [gry, "serve"]}}}
        }))
        .unwrap(),
    )
    .unwrap();

    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let run = |args: &[&str]| {
        Command::new(&opencode)
            .args(args)
            .current_dir(&project)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", &state)
            .output()
            .unwrap_or_else(|error| panic!("opencode {:?}: {error}", args))
    };
    let _ = run(&["service", "set", "port", &port.to_string()]);
    let _ = run(&["debug", "agents"]);
    let mut connected = false;
    let mut log = String::new();
    for _ in 0..40 {
        log = fs::read_to_string(data.join("opencode/log/opencode.log")).unwrap_or_default();
        if log.contains("mcp connected") && log.contains("server=gry tools=18") {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let _ = run(&["service", "stop"]);
    assert!(
        connected,
        "OpenCode did not connect the gry MCP server:\n{log}"
    );
}
