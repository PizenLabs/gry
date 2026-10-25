use gry_core::config::{Config, RuntimeSettings};
use gry_index::pipeline::{IndexRequest, Indexer};
use gry_query::architecture::{get_architecture, ArchitectureRequest};
use gry_query::compare::compare_graphs;
use gry_query::coverage::check_index_coverage;
use gry_query::cypher::{query_graph, QueryRequest};
use gry_query::search::{search_code, search_graph, GraphSearchRequest, SearchCodeRequest};
use gry_query::trace::{trace_path, TraceRequest};
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

fn index_repo(dir: &TempDir, name: &str) -> (Config, String, Store) {
    let root = dir.path().join(name);
    write(
        &root,
        "src/lib.rs",
        r#"
pub struct Service;
impl Service {
    /// Publish an order.
    pub fn publish(&self, id: usize) -> usize { self.validate(id) }
    fn validate(&self, id: usize) -> usize { id }
}
fn main() { let s = Service; s.publish(1); }
"#,
    );
    write(
        &root,
        "web/app.ts",
        "import express from 'express';\nconst app = express();\napp.get('/orders/:id', getOrder);\nexport function getOrder(req: any) { return req; }\n",
    );
    write(
        &root,
        "docs/arch.md",
        "# Architecture\n\nThe `Service` publishes orders.\n\n## Flow\n\nSee [lib](src/lib.rs).\n",
    );
    let cfg = config(dir.path());
    let report = Indexer::new(cfg.clone())
        .index(&IndexRequest::new(root.to_string_lossy()))
        .unwrap();
    let store = Store::open(&cfg.db_path).unwrap();
    (cfg, report.project, store)
}

#[test]
fn semantic_architecture_trace_and_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let (_cfg, project, store) = index_repo(&dir, "one");

    let req = GraphSearchRequest {
        project: project.clone(),
        semantic_query: Some(vec!["publish order".into()]),
        semantic_limit: 10,
        limit: 10,
        ..Default::default()
    };
    let out = search_graph(&store, &req).unwrap();
    assert!(out["semantic"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "publish"));

    let trace = trace_path(
        &store,
        &TraceRequest {
            project: project.clone(),
            function_name: format!("{project}.src.lib.Service.publish"),
            direction: "outbound".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(trace["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "validate"));

    let arch = get_architecture(
        &store,
        &ArchitectureRequest {
            project: project.clone(),
            path: None,
            aspects: vec!["all".into()],
            expected_generation: None,
        },
    )
    .unwrap();
    assert!(arch["languages"].as_array().unwrap().len() >= 2);
    assert!(arch["clusters"]["communities"].as_array().unwrap().len() >= 1);
    assert!(arch["routes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["path"] == "/orders/:id"));

    let code = search_code(
        &store,
        &SearchCodeRequest {
            project: project.clone(),
            pattern: "publish".into(),
            mode: "compact".into(),
            context: 1,
            result_limit: 10,
            match_limit: 5,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(code["results"].as_array().unwrap().len() >= 1);
    assert!(code["results"][0]["matches"][0]["context_start"].is_number());
    let files = search_code(
        &store,
        &SearchCodeRequest {
            project: project.clone(),
            pattern: "publish".into(),
            mode: "files".into(),
            result_limit: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(files["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r.get("line").is_none()));

    let first = query_graph(
        &store,
        &QueryRequest {
            project: project.clone(),
            query: "MATCH (f:Function) RETURN f.qn ORDER BY f.qn".into(),
            max_rows: 1,
            offset: 0,
            cursor: None,
            graph: "code".into(),
        },
    )
    .unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let second = query_graph(
        &store,
        &QueryRequest {
            project: project.clone(),
            query: "MATCH (f:Function) RETURN f.qn ORDER BY f.qn".into(),
            max_rows: 1,
            offset: 0,
            cursor: Some(cursor.clone()),
            graph: "code".into(),
        },
    )
    .unwrap();
    assert_eq!(second["offset"], 1);
    let wrong = query_graph(
        &store,
        &QueryRequest {
            project: project.clone(),
            query: "MATCH (n) RETURN n.qn".into(),
            max_rows: 1,
            offset: 0,
            cursor: Some(cursor),
            graph: "code".into(),
        },
    )
    .unwrap_err();
    assert!(wrong.to_string().contains("different query"));

    let first_page = search_code(
        &store,
        &SearchCodeRequest {
            project: project.clone(),
            pattern: "e".into(),
            mode: "files".into(),
            result_limit: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let code_cursor = first_page["next_cursor"].as_str().unwrap().to_string();
    let second_page = search_code(
        &store,
        &SearchCodeRequest {
            project: project.clone(),
            pattern: "e".into(),
            mode: "files".into(),
            result_limit: 1,
            cursor: Some(code_cursor),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(second_page["result_offset"], 1);

    let coverage = check_index_coverage(
        &store,
        &project,
        &["src/lib.rs".into(), "nope.rs".into()],
        &["src".into()],
        20,
        0,
        20,
        0,
        "none",
    )
    .unwrap();
    assert_eq!(coverage["paths"][0]["status"], "indexed");
    assert_eq!(coverage["paths"][1]["status"], "missing");

    let query = query_graph(
        &store,
        &QueryRequest {
            project: project.clone(),
            query: "MATCH (f:Function) WHERE f.name = 'main' RETURN f.qn".into(),
            max_rows: 10,
            offset: 0,
            cursor: None,
            graph: "code".into(),
        },
    )
    .unwrap();
    assert_eq!(query["rows"].as_array().unwrap().len(), 1);
}

#[test]
fn compare_detects_added_nodes_between_projects() {
    let dir = tempfile::tempdir().unwrap();
    let (_cfg, one, store) = index_repo(&dir, "one");
    let root_two = dir.path().join("two");
    write(&root_two, "src/lib.rs", "pub fn only_here() {}\n");
    let report = Indexer::new(config(dir.path()))
        .index(&IndexRequest::new(root_two.to_string_lossy()))
        .unwrap();
    let out = compare_graphs(&store, &one, &report.project, 100, 100_000).unwrap();
    assert!(out["nodes_added"]["total"].as_i64().unwrap() >= 1);
    assert!(out["nodes_removed"]["total"].as_i64().unwrap() >= 1);
    assert!(out["nodes_added"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "only_here"));
}
