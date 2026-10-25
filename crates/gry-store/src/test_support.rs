use crate::{EdgeSpec, FileHash, ProjectWrite, Store};
use gry_core::types::NewNode;

pub fn test_store() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let nodes = vec![
        node("Module", "main", "p.src.main", "src/main.rs", 1, 20),
        node("Function", "main", "p.src.main.main", "src/main.rs", 2, 12),
        node(
            "Function",
            "parseRequest",
            "p.src.lib.parseRequest",
            "src/lib.rs",
            1,
            20,
        ),
        node(
            "Function",
            "saveOrder",
            "p.src.lib.saveOrder",
            "src/lib.rs",
            22,
            40,
        ),
        node(
            "Function",
            "test_main",
            "p.tests.test_main",
            "tests/test_main.rs",
            1,
            10,
        ),
        node(
            "Function",
            "entryHandler",
            "p.src.main.entryHandler",
            "src/main.rs",
            22,
            30,
        ),
    ];
    let edges = vec![
        EdgeSpec::new("p.src.main.main", "p.src.lib.parseRequest", "CALLS"),
        EdgeSpec::new("p.src.main.main", "p.src.lib.saveOrder", "CALLS"),
        EdgeSpec::new("p.tests.test_main", "p.src.main.main", "CALLS"),
        EdgeSpec::new("p.src.main.entryHandler", "p.src.lib.parseRequest", "CALLS"),
    ];
    let files = vec![
        FileHash {
            rel_path: "src/main.rs".into(),
            sha256: "a".into(),
            mtime_ns: 1,
            size: 20,
        },
        FileHash {
            rel_path: "src/lib.rs".into(),
            sha256: "b".into(),
            mtime_ns: 1,
            size: 40,
        },
        FileHash {
            rel_path: "tests/test_main.rs".into(),
            sha256: "c".into(),
            mtime_ns: 1,
            size: 10,
        },
    ];
    let mut write = ProjectWrite::new("p", "/tmp/p", "full", "gen-1");
    write.nodes = &nodes;
    write.edges = &edges;
    write.files = &files;
    store.publish(&write).unwrap();
    store
}

fn node(label: &str, name: &str, qn: &str, file: &str, start: i32, end: i32) -> NewNode {
    let mut n = NewNode::new(label, name, qn, file, start, end);
    if label == "Function" && qn.starts_with("p.tests.") {
        n = n.with_property("is_test", true);
    }
    n
}
