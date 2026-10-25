//! Opt-in polling watcher. No daemon: the process owns the loop and exits with
//! the user's session.

use crate::discover::{discover, DiscoverOptions, IndexMode};
use crate::pipeline::{IndexRequest, Indexer};
use gry_core::config::Config;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub struct WatchOptions {
    pub repo: String,
    pub interval_secs: u64,
    pub mode: IndexMode,
}

pub fn watch(config: Config, options: WatchOptions) -> gry_core::GryResult<()> {
    let root = config.resolve_index_root(&options.repo)?;
    let mut last_state = tree_state(&root);
    eprintln!(
        "gry watch: {} (every {}s)",
        root.display(),
        options.interval_secs
    );
    loop {
        std::thread::sleep(Duration::from_secs(options.interval_secs.max(1)));
        let state = tree_state(&root);
        if state == last_state {
            continue;
        }
        // Debounce: wait one more interval, then use the settled state.
        std::thread::sleep(Duration::from_secs(options.interval_secs.max(1)));
        let settled = tree_state(&root);
        last_state = settled;
        let mut req = IndexRequest::new(root.to_string_lossy());
        req.mode = options.mode.as_str().to_string();
        let indexer = Indexer::new(config.clone());
        match indexer.index(&req) {
            Ok(report) => {
                eprintln!(
                    "gry watch: indexed {} files (+{} / -{}), {} nodes, {} edges, {} ms",
                    report.files,
                    report.parsed_files,
                    report.deleted_files,
                    report.nodes,
                    report.edges,
                    report.duration_ms
                );
            }
            Err(e) => eprintln!("gry watch: index failed: {e}"),
        }
    }
    #[allow(unreachable_code)]
    {
        Ok(())
    }
}

fn tree_state(root: &Path) -> String {
    // Prefer git's own cheap dirty-state signal.
    if root.join(".git").exists() {
        if let Ok(output) = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["status", "--porcelain=v1", "--branch"])
            .output()
        {
            if output.status.success() {
                let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                if let Ok(head) = Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(["rev-parse", "HEAD"])
                    .output()
                {
                    text.push_str(&String::from_utf8_lossy(&head.stdout));
                }
                return text;
            }
        }
    }
    // Non-git: hash path+size+mtime using the indexer's discovery rules.
    let known: HashMap<String, gry_core::types::FileHash> = HashMap::new();
    let options = DiscoverOptions {
        mode: IndexMode::Full,
        ..Default::default()
    };
    match discover(root, &options, &known, &AtomicBool::new(false)) {
        Ok(discovery) => {
            let mut state = String::new();
            for file in discovery.files {
                state.push_str(&format!(
                    "{}\u{1f}{}\u{1f}{}\n",
                    file.rel_path, file.size, file.mtime_ns
                ));
            }
            state
        }
        Err(_) => String::new(),
    }
}
