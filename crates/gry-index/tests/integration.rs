use gry_core::config::{Config, RuntimeSettings};
use gry_index::discover::IndexMode;
use gry_index::pipeline::{IndexRequest, Indexer};
use gry_store::Store;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn config(dir: &Path) -> Config {
    Config {
        cache_dir: dir.join("cache"),
        db_path: dir.join("cache/graph.db"),
        allowed_root: None,
        workers: 2,
        max_files: None,
        max_source_mb: None,
        settings: RuntimeSettings::default(),
    }
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(
        &root,
        "src/lib.rs",
        r#"
use crate::model::Order;

pub struct Service { count: usize }

impl Service {
    /// Parse an order.
    pub fn handle(&mut self, order: &Order) -> usize {
        self.save(order)
    }
    fn save(&mut self, order: &Order) -> usize { order.id }
}

fn main() {
    let mut s = Service::new();
    s.handle(&Order { id: 1 });
}
"#,
    );
    write(
        &root,
        "src/model.rs",
        "pub struct Order { pub id: usize }\n",
    );
    write(
        &root,
        "app/service.py",
        r#"
from app.model import Order

class OrderService:
    """Handles orders."""
    def handle(self, order: Order) -> int:
        return self.save(order)

    def save(self, order):
        return order.id
"#,
    );
    write(&root, "app/model.py", "class Order:\n    id = 0\n");
    write(
        &root,
        "web/app.ts",
        r#"
import express from 'express';
const app = express();
app.get('/api/orders/:id', getOrder);
export function getOrder(req: any, res: any) {
  publish('orders.created');
  return fetch('/api/orders/:id');
}
"#,
    );
    write(
        &root,
        "Dockerfile",
        "FROM alpine:3.20\nENV PORT=8080\nENTRYPOINT [\"/app/server\"]\n",
    );
    write(
        &root,
        "deploy/api.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\n  labels:\n    app: api\nspec:\n  template:\n    metadata:\n      labels:\n        app: api\n    spec:\n      containers:\n      - name: api\n        image: acme/api:1\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: api\nspec:\n  selector:\n    app: api\n",
    );
    write(
        &root,
        "docs/guide.md",
        "# Guide\n\nSee [model](src/model.rs) and `Service`.\n\n## Setup\n\nRun the service.\n",
    );
    write(
        &root,
        "docs/adr/0001-use-sqlite.md",
        "# ADR-1 Use SQLite\n\nStatus: Accepted\nDate: 2026-01-01\nDeciders: Ada\n\n## Context\n\nNeed storage.\n\n## Decision\n\nUse SQLite.\n\n## Consequences\n\nGood.\n",
    );
    write(
        &root,
        "package.json",
        r#"{"name":"web","dependencies":{"express":"4"}}"#,
    );
    dir
}

#[test]
fn full_index_builds_expected_graph() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    let report = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    assert!(!report.unchanged);
    assert!(report.nodes > 20, "nodes={}", report.nodes);
    assert!(report.edges > 10, "edges={}", report.edges);

    let store = Store::open(&cfg.db_path).unwrap();
    let project = &report.project;
    let service = store
        .resolve_node(project, "Service")
        .unwrap()
        .expect("Service");
    assert_eq!(service.label, "Struct");
    let handle = store
        .nodes_by_file(project, "src/lib.rs")
        .unwrap()
        .into_iter()
        .find(|n| n.name == "handle")
        .expect("handle");
    assert!(handle.qualified_name.ends_with(".Service.handle"));
    let trace = store
        .trace(&{
            let mut p = gry_store::traverse::TraceParams::new(project, &handle.qualified_name);
            p.direction = gry_store::traverse::Direction::Outbound;
            p.depth = 3;
            p.edge_types = vec!["CALLS".to_string()];
            p
        })
        .unwrap()
        .unwrap();
    assert!(trace.hops.iter().any(|h| h.node.name == "save"));
    let route = store
        .nodes_with_label(project, "Route")
        .unwrap()
        .into_iter()
        .find(|n| n.name == "/api/orders/:id")
        .expect("route");
    assert_eq!(route.label, "Route");
    assert!(store
        .nodes_with_label(project, "ADR")
        .unwrap()
        .iter()
        .any(|n| n.properties.get("adr_id").and_then(|v| v.as_str()) == Some("ADR-1")));
    assert!(store
        .nodes_with_label(project, "Resource")
        .unwrap()
        .iter()
        .any(|n| n.name == "api"));
    let labels = store.label_counts(project).unwrap();
    assert!(labels.iter().any(|(l, n)| l == "File" && *n >= 8));
    assert!(labels.iter().any(|(l, _)| l == "Section"));
    assert!(labels.iter().any(|(l, _)| l == "Image"));
}

#[test]
fn unchanged_and_incremental_reindex() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    let first = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let first_nodes = first.nodes;
    let second = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    assert!(second.unchanged);
    assert_eq!(second.parsed_files, 0);
    assert_eq!(second.nodes, first_nodes);

    // Edit one file: only that file should be parsed.
    write(
        &root,
        "src/lib.rs",
        r#"
pub struct Service { count: usize }
impl Service {
    pub fn handle(&mut self) -> usize { self.save() }
    fn save(&mut self) -> usize { 42 }
}
fn main() { let mut s = Service { count: 0 }; s.handle(); }
"#,
    );
    let third = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    assert!(!third.unchanged);
    assert_eq!(third.parsed_files, 1, "only changed file should parse");
    assert!(third.reused_files >= 8);

    // Delete a file: its nodes must disappear.
    fs::remove_file(root.join("app/model.py")).unwrap();
    let fourth = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    assert_eq!(fourth.deleted_files, 1);
    let store = Store::open(&cfg.db_path).unwrap();
    assert!(store
        .nodes_by_file(&fourth.project, "app/model.py")
        .unwrap()
        .is_empty());
}

#[test]
fn filtered_and_fast_modes() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    let mut req = IndexRequest::new(root.to_string_lossy());
    req.mode = "fast".to_string();
    let report = indexer.index(&req).unwrap();
    assert_eq!(report.mode, IndexMode::Fast.as_str());
    let store = Store::open(&cfg.db_path).unwrap();
    // docs/ is skipped by fast discovery.
    assert!(store
        .nodes_by_file(&report.project, "docs/guide.md")
        .unwrap()
        .is_empty());
}

#[test]
fn artifact_round_trip() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    let mut req = IndexRequest::new(root.to_string_lossy());
    req.persistence = true;
    let report = indexer.index(&req).unwrap();
    let artifact = report.artifact.expect("artifact");
    assert!(Path::new(&artifact).is_file());
    let mut store = Store::open(&cfg.db_path).unwrap();
    store.delete_project(&report.project).unwrap();
    let meta = store.import_artifact(&artifact).unwrap();
    assert_eq!(meta.project, report.project);
    assert!(store
        .resolve_node(&meta.project, "Service")
        .unwrap()
        .is_some());
}

#[test]
fn git_history_coupling_edges() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(&root, "src/a.rs", "pub fn a() {}\n");
    write(&root, "src/b.rs", "pub fn b() {}\n");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
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
    for i in 0..3 {
        write(
            &root,
            "src/a.rs",
            &format!("pub fn a() {{ let _ = {i}; }}\n"),
        );
        write(
            &root,
            "src/b.rs",
            &format!("pub fn b() {{ let _ = {i}; }}\n"),
        );
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("change {i}")]);
    }
    let cfg = config(dir.path());
    let report = Indexer::new(cfg.clone())
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let store = Store::open(&cfg.db_path).unwrap();
    let edges = store.all_edges(&report.project).unwrap();
    assert!(edges.iter().any(|e| e.edge_type == "FILE_CHANGES_WITH"));
}

fn graph_signature(store: &Store, project: &str) -> (Vec<String>, Vec<String>) {
    let nodes = store.all_nodes(project).unwrap();
    let mut by_id = std::collections::HashMap::new();
    let mut node_rows = Vec::new();
    for node in &nodes {
        by_id.insert(node.id, node.qualified_name.clone());
        node_rows.push(format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            node.label, node.qualified_name, node.file_path, node.start_line
        ));
    }
    let mut edge_rows = Vec::new();
    for edge in store.all_edges(project).unwrap() {
        if matches!(
            edge.edge_type.as_str(),
            "SIMILAR_TO" | "SEMANTICALLY_RELATED" | "FILE_CHANGES_WITH"
        ) {
            continue;
        }
        let source = by_id.get(&edge.source_id).cloned().unwrap_or_default();
        let target = by_id.get(&edge.target_id).cloned().unwrap_or_default();
        edge_rows.push(format!(
            "{}\u{1f}{}\u{1f}{}",
            source, edge.edge_type, target
        ));
    }
    node_rows.sort();
    edge_rows.sort();
    (node_rows, edge_rows)
}

#[test]
fn incremental_matches_clean_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(
        &root,
        "src/lib.rs",
        r#"
pub struct Engine;
impl Engine {
    pub fn run(&self, n: usize) -> usize { helper(n) }
}
pub fn helper(n: usize) -> usize { n + 1 }
fn main() { let e = Engine; e.run(1); }
"#,
    );
    write(&root, "src/util.rs", "pub fn util() -> usize { 1 }\n");
    write(
        &root,
        "src/caller.rs",
        "pub fn caller() { crate::util::util(); }\n",
    );
    write(
        &root,
        "tests/test_engine.rs",
        "#[test]\nfn test_run() { let e = crate::Engine; assert_eq!(e.run(1), 2); }\n",
    );
    write(
        &root,
        "docs/guide.md",
        "# Guide\n\nSee [lib](src/lib.rs).\n",
    );
    write(&root, "notes.txt", "unrelated\n");

    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // 1. Comment-only edit: nodes_only path.
    write(
        &root,
        "src/lib.rs",
        r#"
pub struct Engine;
impl Engine {
    pub fn run(&self, n: usize) -> usize { helper(n) }
}
pub fn helper(n: usize) -> usize { n + 1 }
fn main() { let e = Engine; e.run(1); }
// trailing comment
"#,
    );
    // 2. Body literal change: still structurally identical.
    write(&root, "src/util.rs", "pub fn util() -> usize { 2 }\n");
    // 3. Rename a function: structural.
    write(&root, "src/caller.rs", "pub fn caller() { crate::util::util(); }\npub fn extra() -> usize { crate::util::util() }\n");
    // 4. Add a file that calls an existing symbol.
    write(
        &root,
        "src/added.rs",
        "pub fn added() -> usize { crate::util::util() }\n",
    );
    // 5. Delete a file.
    write(&root, "notes.txt", "still irrelevant\n");
    let _ = std::fs::remove_file(root.join("notes.txt"));
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    let store = Store::open(&cfg.db_path).unwrap();
    let project = store.projects().unwrap()[0].name.clone();
    let incremental = graph_signature(&store, &project);
    drop(store);

    // Clean rebuild in an independent cache.
    let clean_dir = tempfile::tempdir().unwrap();
    let clean_cfg = config(clean_dir.path());
    let clean_report = Indexer::new(clean_cfg.clone())
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let clean_store = Store::open(&clean_cfg.db_path).unwrap();
    let clean = graph_signature(&clean_store, &clean_report.project);

    let node_diff: Vec<_> = clean
        .0
        .iter()
        .filter(|n| !incremental.0.contains(n))
        .cloned()
        .collect();
    let extra_nodes: Vec<_> = incremental
        .0
        .iter()
        .filter(|n| !clean.0.contains(n))
        .cloned()
        .collect();
    let edge_diff: Vec<_> = clean
        .1
        .iter()
        .filter(|e| !incremental.1.contains(e))
        .cloned()
        .collect();
    let extra_edges: Vec<_> = incremental
        .1
        .iter()
        .filter(|e| !clean.1.contains(e))
        .cloned()
        .collect();
    assert!(
        node_diff.is_empty() && extra_nodes.is_empty(),
        "node mismatch\nmissing={node_diff:?}\nextra={extra_nodes:?}"
    );
    assert!(
        edge_diff.is_empty() && extra_edges.is_empty(),
        "edge mismatch\nmissing={edge_diff:?}\nextra={extra_edges:?}"
    );
}

#[test]
fn incremental_c_shared_module_matches_clean_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(
        &root,
        "src/util.h",
        "#ifndef UTIL_H\n#define UTIL_H\nint add(int a, int b);\nint twice(int a);\n#endif\n",
    );
    write(
        &root,
        "src/util.c",
        "#include \"util.h\"\nint add(int a, int b) { return a + b; }\nint twice(int a) { return add(a, a); }\n",
    );
    write(
        &root,
        "src/main.c",
        "#include \"util.h\"\nint main(void) { return twice(2) == 4 ? 0 : 1; }\n",
    );
    write(
        &root,
        "src/extra.c",
        "#include \"util.h\"\nint extra(void) { return add(1, 2); }\n",
    );

    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // Body-only edit in one translation unit (shared module: util.c/util.h).
    write(
        &root,
        "src/util.c",
        "#include \"util.h\"\nint add(int a, int b) { return a + b + 0; }\nint twice(int a) { return add(a, a); }\n// note\n",
    );
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // Rename a function and update callers.
    write(
        &root,
        "src/util.h",
        "#ifndef UTIL_H\n#define UTIL_H\nint add(int a, int b);\nint double_it(int a);\n#endif\n",
    );
    write(
        &root,
        "src/util.c",
        "#include \"util.h\"\nint add(int a, int b) { return a + b; }\nint double_it(int a) { return add(a, a); }\n",
    );
    write(
        &root,
        "src/main.c",
        "#include \"util.h\"\nint main(void) { return double_it(2) == 4 ? 0 : 1; }\n",
    );

    // Add a caller and remove another file.
    write(
        &root,
        "src/added.c",
        "#include \"util.h\"\nint added(void) { return add(3, 4); }\n",
    );
    let _ = std::fs::remove_file(root.join("src/extra.c"));
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    let store = Store::open(&cfg.db_path).unwrap();
    let project = store.projects().unwrap()[0].name.clone();
    let incremental = graph_signature(&store, &project);
    drop(store);

    let clean_dir = tempfile::tempdir().unwrap();
    let clean_cfg = config(clean_dir.path());
    let clean_report = Indexer::new(clean_cfg.clone())
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let clean_store = Store::open(&clean_cfg.db_path).unwrap();
    let clean = graph_signature(&clean_store, &clean_report.project);

    let node_diff: Vec<_> = clean
        .0
        .iter()
        .filter(|n| !incremental.0.contains(n))
        .cloned()
        .collect();
    let extra_nodes: Vec<_> = incremental
        .0
        .iter()
        .filter(|n| !clean.0.contains(n))
        .cloned()
        .collect();
    let edge_diff: Vec<_> = clean
        .1
        .iter()
        .filter(|e| !incremental.1.contains(e))
        .cloned()
        .collect();
    let extra_edges: Vec<_> = incremental
        .1
        .iter()
        .filter(|e| !clean.1.contains(e))
        .cloned()
        .collect();
    assert!(
        node_diff.is_empty() && extra_nodes.is_empty(),
        "node mismatch\nmissing={node_diff:?}\nextra={extra_nodes:?}"
    );
    assert!(
        edge_diff.is_empty() && extra_edges.is_empty(),
        "edge mismatch\nmissing={edge_diff:?}\nextra={extra_edges:?}"
    );
}

#[test]
fn incremental_resolves_late_imports_and_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(
        &root,
        "app/use.py",
        "from app.helpers import compute\n\ndef run(n):\n    return compute(n)\n",
    );
    write(&root, "app/__init__.py", "");
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // Add the previously missing module: existing unresolved import + call
    // must now resolve without a full rebuild.
    write(
        &root,
        "app/helpers.py",
        "def compute(n):\n    return n * 2\n",
    );
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // Config key change and a documentation reference update.
    write(&root, "config/app.json", "{\"compute_timeout\": 30}\n");
    write(&root, "docs/guide.md", "# Guide\n\nUse `run`.\n");
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // Remove the module again: import and call edges must disappear.
    let _ = std::fs::remove_file(root.join("app/helpers.py"));
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    let store = Store::open(&cfg.db_path).unwrap();
    let project = store.projects().unwrap()[0].name.clone();
    let incremental = graph_signature(&store, &project);
    drop(store);

    let clean_dir = tempfile::tempdir().unwrap();
    let clean_cfg = config(clean_dir.path());
    let clean_report = Indexer::new(clean_cfg.clone())
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let clean_store = Store::open(&clean_cfg.db_path).unwrap();
    let clean = graph_signature(&clean_store, &clean_report.project);
    let node_diff: Vec<_> = clean
        .0
        .iter()
        .filter(|n| !incremental.0.contains(n))
        .cloned()
        .collect();
    let extra_nodes: Vec<_> = incremental
        .0
        .iter()
        .filter(|n| !clean.0.contains(n))
        .cloned()
        .collect();
    let edge_diff: Vec<_> = clean
        .1
        .iter()
        .filter(|e| !incremental.1.contains(e))
        .cloned()
        .collect();
    let extra_edges: Vec<_> = incremental
        .1
        .iter()
        .filter(|e| !clean.1.contains(e))
        .cloned()
        .collect();
    assert!(
        node_diff.is_empty() && extra_nodes.is_empty(),
        "node mismatch\nmissing={node_diff:?}\nextra={extra_nodes:?}"
    );
    assert!(
        edge_diff.is_empty() && extra_edges.is_empty(),
        "edge mismatch\nmissing={edge_diff:?}\nextra={extra_edges:?}"
    );
}

#[test]
fn failed_or_cancelled_run_preserves_previous_generation() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let mut cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    let first = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let store = Store::open(&cfg.db_path).unwrap();
    let generation = store.project(&first.project).unwrap().unwrap().generation;
    let nodes_before = store.counts(&first.project).unwrap().0;
    drop(store);

    // Budget failure happens before publication.
    cfg.settings.index_max_memory_mb = Some(1);
    let mut budget_request = IndexRequest::new(root.to_string_lossy());
    budget_request.mode = "fast".to_string();
    let err = Indexer::new(cfg.clone())
        .index(&budget_request)
        .unwrap_err();
    assert!(err.to_string().contains("memory budget"));
    let store = Store::open(&cfg.db_path).unwrap();
    let record = store.project(&first.project).unwrap().unwrap();
    assert_eq!(record.generation, generation);
    assert_eq!(store.counts(&first.project).unwrap().0, nodes_before);
    drop(store);

    // Cancellation before parsing also leaves the previous generation.
    cfg.settings.index_max_memory_mb = None;
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let err = Indexer::new(cfg.clone())
        .index_with_cancel(&IndexRequest::new(root.to_string_lossy()), cancel)
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("cancel"));
    let store = Store::open(&cfg.db_path).unwrap();
    assert_eq!(
        store.project(&first.project).unwrap().unwrap().generation,
        generation
    );
}

#[test]
fn deleting_symbol_free_module_re_resolves_importers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    write(&root, "pkg/blank.py", "# intentionally empty\n");
    write(&root, "pkg/__init__.py", "");
    write(
        &root,
        "pkg/consumer.py",
        "import pkg.blank\n\ndef run():\n    return 1\n",
    );
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let _ = std::fs::remove_file(root.join("pkg/blank.py"));
    indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    let store = Store::open(&cfg.db_path).unwrap();
    let project = store.projects().unwrap()[0].name.clone();
    let incremental = graph_signature(&store, &project);
    drop(store);
    let clean_dir = tempfile::tempdir().unwrap();
    let clean_cfg = config(clean_dir.path());
    let clean_report = Indexer::new(clean_cfg.clone())
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let clean_store = Store::open(&clean_cfg.db_path).unwrap();
    let clean = graph_signature(&clean_store, &clean_report.project);
    let edge_diff: Vec<_> = clean
        .1
        .iter()
        .filter(|e| !incremental.1.contains(e))
        .cloned()
        .collect();
    let extra_edges: Vec<_> = incremental
        .1
        .iter()
        .filter(|e| !clean.1.contains(e))
        .cloned()
        .collect();
    assert!(
        edge_diff.is_empty() && extra_edges.is_empty(),
        "edge mismatch\nmissing={edge_diff:?}\nextra={extra_edges:?}"
    );
}

#[test]
fn cross_repo_links_routes_and_channels() {
    let dir = fixture();
    let root = dir.path().join("repo");
    let cfg = config(dir.path());
    let indexer = Indexer::new(cfg.clone());
    let a = indexer
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();

    // Second repository with a matching route and channel.
    let root_b = dir.path().join("repo-b");
    write(
        &root_b,
        "server.py",
        "def get_order(req):\n    publish('orders.created', req)\n    return req\n",
    );
    write(
        &root_b,
        "routes.py",
        "from flask import Flask\napp = Flask(__name__)\n@app.get('/api/orders/:id')\ndef get_order(req):\n    return req\n",
    );
    let mut req_b = IndexRequest::new(root_b.to_string_lossy());
    req_b.name = Some("repo-b".to_string());
    let b = indexer.index(&req_b).unwrap();

    let mut cross = IndexRequest::new(root.to_string_lossy());
    cross.mode = "cross-repo-intelligence".to_string();
    cross.target_projects = vec![b.project.clone()];
    let cross_report = indexer.index(&cross).unwrap();
    let cross = cross_report.cross_repo.unwrap();
    assert_eq!(cross["status"], "ran");
    assert!(cross["total_cross_edges"].as_i64().unwrap() >= 1);
    let store = Store::open(&cfg.db_path).unwrap();
    let edges = store.all_edges(&a.project).unwrap();
    assert!(edges.iter().any(|e| e.edge_type.starts_with("CROSS_")));
}
