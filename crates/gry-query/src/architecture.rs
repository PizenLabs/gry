//! Bounded static architecture analysis.

use gry_core::types::{labels, Edge, Node};
use gry_store::Store;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct ArchitectureRequest {
    pub project: String,
    pub path: Option<String>,
    pub aspects: Vec<String>,
    pub expected_generation: Option<String>,
}

pub fn get_architecture(store: &Store, req: &ArchitectureRequest) -> gry_core::GryResult<Value> {
    let record = store.require_project(&req.project)?;
    if let Some(expected) = &req.expected_generation {
        if expected != &record.generation {
            return Err(gry_core::GryError::invalid(format!(
                "generation mismatch: expected {expected}, current {}",
                record.generation
            )));
        }
    }
    let aspects = normalize_aspects(&req.aspects)?;
    if aspects.iter().any(|a| a == "system_structure") {
        return Err(gry_core::GryError::unsupported(
            "system_structure is a Graph Viewer projection and is not part of gry",
        ));
    }
    let all_nodes = store.all_nodes(&req.project)?;
    let all_edges = store.all_edges(&req.project)?;
    let nodes = filter_nodes(&all_nodes, req.path.as_deref());
    let node_ids: HashSet<i64> = nodes.iter().map(|n| n.id).collect();
    let edges: Vec<&Edge> = all_edges
        .iter()
        .filter(|e| node_ids.contains(&e.source_id) && node_ids.contains(&e.target_id))
        .collect();
    let by_id: HashMap<i64, &Node> = nodes.iter().map(|n| (n.id, *n)).collect();

    let mut out = Map::new();
    out.insert("project".into(), json!(req.project));
    out.insert("generation".into(), json!(record.generation));
    out.insert("path".into(), json!(req.path));
    out.insert("aspects".into(), json!(aspects));
    out.insert("nodes".into(), json!(nodes.len()));
    out.insert("edges".into(), json!(edges.len()));
    out.insert(
        "caveat".into(),
        json!("Static analysis of indexed evidence; unresolved references and coverage gaps still exist."),
    );

    let wants_all = aspects.iter().any(|a| a == "all");
    let wants = |name: &str| wants_all || aspects.iter().any(|a| a == name);

    if wants("languages") || wants("overview") {
        out.insert("languages".into(), languages(&nodes));
    }
    if wants("packages") || wants("overview") {
        out.insert("packages".into(), packages(&nodes, &edges, &by_id));
    }
    if wants("entry_points") || wants("overview") {
        out.insert("entry_points".into(), entry_points(&nodes, &edges, &by_id));
    }
    if wants("routes") || wants("overview") {
        out.insert("routes".into(), routes(&nodes));
    }
    if wants("hotspots") || wants("overview") {
        out.insert("hotspots".into(), hotspots(&nodes, &edges, &by_id));
    }
    if wants("structure") {
        out.insert("structure".into(), structure(&nodes));
    }
    if wants("dependencies") {
        out.insert("dependencies".into(), dependencies(&edges, &by_id));
    }
    if wants("boundaries") {
        out.insert("boundaries".into(), boundaries(&edges, &by_id));
    }
    if wants("layers") {
        out.insert("layers".into(), layers(&nodes, &edges, &by_id));
    }
    if wants("file_tree") {
        out.insert("file_tree".into(), file_tree(&nodes));
    }
    if wants("clusters") {
        out.insert("clusters".into(), clusters(&nodes, &edges, &by_id));
    }
    if wants("cycles") {
        out.insert("cycles".into(), cycles(&nodes, &edges, &by_id));
    }
    Ok(Value::Object(out))
}

fn normalize_aspects(aspects: &[String]) -> gry_core::GryResult<Vec<String>> {
    const VALID: &[&str] = &[
        "all",
        "overview",
        "structure",
        "dependencies",
        "routes",
        "languages",
        "packages",
        "entry_points",
        "hotspots",
        "boundaries",
        "layers",
        "file_tree",
        "clusters",
        "cycles",
        "system_structure",
    ];
    if aspects.is_empty() {
        return Ok(vec![
            "languages".to_string(),
            "packages".to_string(),
            "entry_points".to_string(),
        ]);
    }
    for aspect in aspects {
        if !VALID.contains(&aspect.as_str()) {
            return Err(gry_core::GryError::invalid(format!(
                "invalid aspect {aspect}; valid: {}",
                VALID.join(", ")
            )));
        }
    }
    Ok(aspects.to_vec())
}

fn filter_nodes<'a>(nodes: &'a [Node], path: Option<&str>) -> Vec<&'a Node> {
    let Some(path) = path else {
        return nodes.iter().collect();
    };
    let prefix = path.trim_start_matches("./").trim_start_matches('/');
    nodes
        .iter()
        .filter(|n| {
            n.file_path.starts_with(prefix)
                || n.label == labels::PROJECT && prefix.is_empty()
                || n.file_path.is_empty() && prefix.is_empty()
        })
        .collect()
}

fn component(node: &Node) -> String {
    if node.file_path.is_empty() {
        return format!("[{0}]", node.label);
    }
    node.file_path.split('/').next().unwrap_or(".").to_string()
}

fn languages(nodes: &[&Node]) -> Value {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut lines: BTreeMap<String, i64> = BTreeMap::new();
    for node in nodes {
        if node.label == labels::FILE {
            let lang = node
                .property_str("language")
                .unwrap_or("unknown")
                .to_string();
            *counts.entry(lang.clone()).or_insert(0) += 1;
            *lines.entry(lang).or_insert(0) += (node.end_line.max(0)) as i64;
        }
    }
    let mut rows: Vec<Value> = counts
        .into_iter()
        .map(|(language, files)| {
            json!({
                "language": language,
                "files": files,
                "lines": lines.get(&language).copied().unwrap_or(0),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b["files"]
            .as_i64()
            .cmp(&a["files"].as_i64())
            .then(a["language"].as_str().cmp(&b["language"].as_str()))
    });
    Value::Array(rows)
}

fn packages(nodes: &[&Node], edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let mut rows = Vec::new();
    for node in nodes.iter().filter(|n| n.label == labels::PACKAGE) {
        let mut dependents = 0;
        let mut dependencies = 0;
        for edge in edges {
            if edge.target_id == node.id
                && matches!(edge.edge_type.as_str(), "IMPORTS" | "DEPENDS_ON")
            {
                dependents += 1;
            }
            if edge.source_id == node.id && edge.edge_type == "DEPENDS_ON" {
                dependencies += 1;
            }
        }
        rows.push(json!({
            "name": node.name,
            "qn": node.qualified_name,
            "ecosystem": node.property_str("ecosystem"),
            "external": node.property_bool("external").unwrap_or(false),
            "dependents": dependents,
            "dependencies": dependencies,
            "manifest": node.file_path,
        }));
    }
    // Derive top-level components when no Package nodes exist.
    if rows.is_empty() {
        let mut comps: BTreeMap<String, i64> = BTreeMap::new();
        for node in nodes.iter().filter(|n| n.label == labels::FILE) {
            *comps.entry(component(node)).or_insert(0) += 1;
        }
        rows = comps
            .into_iter()
            .map(|(name, files)| json!({"name": name, "files": files, "derived": true}))
            .collect();
    }
    let _ = by_id;
    rows.sort_by(|a, b| {
        b["dependents"]
            .as_i64()
            .cmp(&a["dependents"].as_i64())
            .then(a["name"].as_str().cmp(&b["name"].as_str()))
    });
    Value::Array(rows.into_iter().take(50).collect())
}

fn entry_points(nodes: &[&Node], edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let mut in_deg: HashMap<i64, i64> = HashMap::new();
    let mut out_deg: HashMap<i64, i64> = HashMap::new();
    for edge in edges {
        if edge.edge_type == "CALLS" {
            *in_deg.entry(edge.target_id).or_insert(0) += 1;
            *out_deg.entry(edge.source_id).or_insert(0) += 1;
        }
    }
    let mut rows = Vec::new();
    for node in nodes {
        if !labels::is_callable(&node.label) {
            continue;
        }
        let marked = node.property_bool("is_entry_point").unwrap_or(false);
        let inferred = in_deg.get(&node.id).copied().unwrap_or(0) == 0
            && out_deg.get(&node.id).copied().unwrap_or(0) > 0;
        if marked || inferred || matches!(node.name.as_str(), "main" | "_start" | "init") {
            rows.push(json!({
                "qn": node.qualified_name,
                "name": node.name,
                "label": node.label,
                "file": node.file_path,
                "lines": [node.start_line, node.end_line],
                "marked": marked,
                "inferred": inferred,
            }));
        }
    }
    rows.sort_by(|a, b| a["qn"].as_str().cmp(&b["qn"].as_str()));
    let _ = by_id;
    Value::Array(rows.into_iter().take(50).collect())
}

fn routes(nodes: &[&Node]) -> Value {
    let mut rows: Vec<Value> = nodes
        .iter()
        .filter(|n| n.label == labels::ROUTE)
        .map(|n| {
            json!({
                "qn": n.qualified_name,
                "path": n.name,
                "method": n.property_str("method"),
                "source": n.property_str("source"),
                "file": n.file_path,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a["path"]
            .as_str()
            .cmp(&b["path"].as_str())
            .then(a["method"].as_str().cmp(&b["method"].as_str()))
    });
    Value::Array(rows.into_iter().take(100).collect())
}

fn degree_maps(edges: &[&Edge]) -> (HashMap<i64, i64>, HashMap<i64, i64>) {
    let mut in_deg = HashMap::new();
    let mut out_deg = HashMap::new();
    for edge in edges {
        if !matches!(
            edge.edge_type.as_str(),
            "CALLS"
                | "USAGE"
                | "CALL_REFERENCE"
                | "INHERITS"
                | "IMPLEMENTS"
                | "IMPORTS"
                | "HTTP_CALLS"
                | "GRPC_CALLS"
                | "ASYNC_CALLS"
                | "TESTS"
        ) {
            continue;
        }
        *in_deg.entry(edge.target_id).or_insert(0) += 1;
        *out_deg.entry(edge.source_id).or_insert(0) += 1;
    }
    (in_deg, out_deg)
}

fn hotspots(nodes: &[&Node], edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let (in_deg, out_deg) = degree_maps(edges);
    let mut rows: Vec<Value> = nodes
        .iter()
        .filter(|n| labels::is_callable(&n.label) || labels::is_type_like(&n.label))
        .map(|n| {
            let i = in_deg.get(&n.id).copied().unwrap_or(0);
            let o = out_deg.get(&n.id).copied().unwrap_or(0);
            json!({
                "qn": n.qualified_name,
                "name": n.name,
                "label": n.label,
                "file": n.file_path,
                "in": i,
                "out": o,
                    "score": i * 2 + o,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b["score"]
            .as_i64()
            .cmp(&a["score"].as_i64())
            .then(a["qn"].as_str().cmp(&b["qn"].as_str()))
    });
    let _ = by_id;
    Value::Array(rows.into_iter().take(15).collect())
}

fn structure(nodes: &[&Node]) -> Value {
    let mut labels: BTreeMap<String, i64> = BTreeMap::new();
    let mut dirs: BTreeMap<String, i64> = BTreeMap::new();
    for node in nodes {
        *labels.entry(node.label.clone()).or_insert(0) += 1;
        if node.label == labels::FILE {
            *dirs.entry(component(node)).or_insert(0) += 1;
        }
    }
    json!({
        "labels": labels.into_iter().map(|(l,n)| json!({"label":l,"count":n})).collect::<Vec<_>>(),
        "directories": dirs.into_iter().map(|(d,n)| json!({"directory":d,"files":n})).collect::<Vec<_>>(),
        "modules": nodes.iter().filter(|n| n.label == labels::MODULE).count(),
        "files": nodes.iter().filter(|n| n.label == labels::FILE).count(),
    })
}

fn dependencies(edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
    for edge in edges {
        if !matches!(edge.edge_type.as_str(), "IMPORTS" | "DEPENDS_ON") {
            continue;
        }
        let Some(source) = by_id.get(&edge.source_id) else {
            continue;
        };
        let Some(target) = by_id.get(&edge.target_id) else {
            continue;
        };
        let from = component(source);
        let to = if target.label == labels::PACKAGE {
            target.name.clone()
        } else {
            component(target)
        };
        if from == to {
            continue;
        }
        *counts.entry((from, to)).or_insert(0) += 1;
    }
    let mut rows: Vec<Value> = counts
        .into_iter()
        .map(|((from, to), count)| json!({"from": from, "to": to, "count": count}))
        .collect();
    rows.sort_by(|a, b| {
        b["count"]
            .as_i64()
            .cmp(&a["count"].as_i64())
            .then(a["from"].as_str().cmp(&b["from"].as_str()))
            .then(a["to"].as_str().cmp(&b["to"].as_str()))
    });
    Value::Array(rows.into_iter().take(100).collect())
}

fn boundaries(edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
    for edge in edges {
        let Some(source) = by_id.get(&edge.source_id) else {
            continue;
        };
        let Some(target) = by_id.get(&edge.target_id) else {
            continue;
        };
        let from = component(source);
        let to = component(target);
        if from == to {
            continue;
        }
        *counts.entry((from, to)).or_insert(0) += 1;
    }
    let mut rows: Vec<Value> = counts
        .into_iter()
        .map(|((from, to), count)| json!({"from": from, "to": to, "count": count}))
        .collect();
    rows.sort_by(|a, b| {
        b["count"]
            .as_i64()
            .cmp(&a["count"].as_i64())
            .then(a["from"].as_str().cmp(&b["from"].as_str()))
            .then(a["to"].as_str().cmp(&b["to"].as_str()))
    });
    Value::Array(rows.into_iter().take(100).collect())
}

fn layer_of(component: &str) -> &'static str {
    let c = component.to_ascii_lowercase();
    if c.contains("test") || c.contains("spec") || c.contains("fixture") {
        "tests"
    } else if c.contains("doc") {
        "docs"
    } else if c.contains("infra")
        || c.contains("deploy")
        || c.contains("k8s")
        || c.contains("terraform")
        || c.contains("helm")
    {
        "infrastructure"
    } else if c.contains("api")
        || c.contains("route")
        || c.contains("controller")
        || c.contains("handler")
        || c.contains("web")
        || c.contains("ui")
        || c.contains("frontend")
    {
        "presentation"
    } else if c.contains("service")
        || c.contains("domain")
        || c.contains("core")
        || c.contains("usecase")
        || c.contains("business")
    {
        "domain"
    } else if c.contains("db")
        || c.contains("store")
        || c.contains("repository")
        || c.contains("persistence")
        || c.contains("model")
    {
        "data"
    } else if c.contains("script") || c.contains("tool") {
        "tooling"
    } else {
        "application"
    }
}

fn layers(nodes: &[&Node], edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let mut counts: BTreeMap<&'static str, i64> = BTreeMap::new();
    for node in nodes {
        if node.label == labels::FILE {
            *counts.entry(layer_of(&component(node))).or_insert(0) += 1;
        }
    }
    let mut cross: BTreeMap<(&'static str, &'static str), i64> = BTreeMap::new();
    for edge in edges {
        let Some(source) = by_id.get(&edge.source_id) else {
            continue;
        };
        let Some(target) = by_id.get(&edge.target_id) else {
            continue;
        };
        let a = layer_of(&component(source));
        let b = layer_of(&component(target));
        if a != b {
            *cross.entry((a, b)).or_insert(0) += 1;
        }
    }
    json!({
        "layers": counts.into_iter().map(|(l,n)| json!({"layer":l,"files":n})).collect::<Vec<_>>(),
        "cross_layer_edges": cross.into_iter().map(|((a,b),n)| json!({"from":a,"to":b,"count":n})).collect::<Vec<_>>(),
        "heuristic": true,
    })
}

fn file_tree(nodes: &[&Node]) -> Value {
    #[derive(Default)]
    struct Dir {
        files: Vec<Value>,
        children: BTreeMap<String, Dir>,
    }
    fn insert(dir: &mut Dir, parts: &[&str], file: Value) {
        if parts.is_empty() {
            dir.files.push(file);
            return;
        }
        insert(
            dir.children.entry(parts[0].to_string()).or_default(),
            &parts[1..],
            file,
        );
    }
    fn render(name: &str, dir: &Dir) -> Value {
        json!({
            "name": name,
            "files": dir.files,
            "children": dir.children.iter().map(|(n,d)| render(n,d)).collect::<Vec<_>>(),
        })
    }
    let mut root = Dir::default();
    for node in nodes.iter().filter(|n| n.label == labels::FILE) {
        let parts: Vec<&str> = node.file_path.split('/').collect();
        insert(
            &mut root,
            &parts[..parts.len().saturating_sub(1)],
            json!({
                "name": node.name,
                "file": node.file_path,
                "nodes": node.property_i64("nodes").unwrap_or(0),
                "language": node.property_str("language"),
            }),
        );
    }
    render("", &root)
}

fn callable_adjacency(
    edges: &[&Edge],
    by_id: &HashMap<i64, &Node>,
) -> BTreeMap<i64, BTreeMap<i64, i64>> {
    let mut adj: BTreeMap<i64, BTreeMap<i64, i64>> = BTreeMap::new();
    for edge in edges {
        if edge.edge_type != "CALLS" {
            continue;
        }
        let Some(source) = by_id.get(&edge.source_id) else {
            continue;
        };
        let Some(target) = by_id.get(&edge.target_id) else {
            continue;
        };
        if !labels::is_callable(&source.label) || !labels::is_callable(&target.label) {
            continue;
        }
        *adj.entry(source.id)
            .or_default()
            .entry(target.id)
            .or_insert(0) += 1;
        *adj.entry(target.id)
            .or_default()
            .entry(source.id)
            .or_insert(0) += 1;
    }
    adj
}

fn clusters(_nodes: &[&Node], edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let adj = callable_adjacency(edges, by_id);
    let mut ids: Vec<i64> = adj.keys().copied().collect();
    ids.sort_unstable();
    if ids.is_empty() {
        return json!({"communities": [], "modularity": 0.0});
    }
    let mut community: HashMap<i64, usize> =
        ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let degree: HashMap<i64, i64> = adj
        .iter()
        .map(|(id, neighbors)| (*id, neighbors.values().sum::<i64>()))
        .collect();
    let total_weight: i64 = adj.values().flat_map(|m| m.values()).sum::<i64>();
    if total_weight == 0 {
        return json!({"communities": [], "modularity": 0.0});
    }
    for _ in 0..10 {
        let mut changed = false;
        for id in &ids {
            let mut weights: BTreeMap<usize, i64> = BTreeMap::new();
            if let Some(neighbors) = adj.get(id) {
                for (other, weight) in neighbors {
                    *weights
                        .entry(*community.get(other).unwrap_or(&0))
                        .or_insert(0) += *weight;
                }
            }
            let current = *community.get(id).unwrap_or(&0);
            let best = weights
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(c, _)| *c)
                .unwrap_or(current);
            if best != current {
                community.insert(*id, best);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut groups: BTreeMap<usize, Vec<i64>> = BTreeMap::new();
    for id in &ids {
        groups
            .entry(*community.get(id).unwrap_or(&0))
            .or_default()
            .push(*id);
    }
    let m = total_weight as f64 / 2.0;
    let mut modularity = 0.0;
    for members in groups.values() {
        if members.is_empty() {
            continue;
        }
        let member_set: HashSet<i64> = members.iter().copied().collect();
        let mut internal = 0i64;
        let mut degree_sum = 0i64;
        for id in members {
            degree_sum += degree.get(id).copied().unwrap_or(0);
            if let Some(neighbors) = adj.get(id) {
                for (other, w) in neighbors {
                    if member_set.contains(other) {
                        internal += *w;
                    }
                }
            }
        }
        let internal = internal as f64 / (2.0 * m);
        let expected = (degree_sum as f64 / (2.0 * m)).powi(2);
        modularity += internal - expected;
    }
    let mut rows: Vec<Value> = groups
        .into_iter()
        .filter(|(_, members)| members.len() >= 2)
        .map(|(cid, members)| {
            let mut members_qn: Vec<Value> = members
                .iter()
                .filter_map(|id| by_id.get(id))
                .map(|n| json!({"qn": n.qualified_name, "name": n.name, "file": n.file_path}))
                .collect();
            members_qn.sort_by(|a, b| a["qn"].as_str().cmp(&b["qn"].as_str()));
            let member_set: HashSet<i64> = members.iter().copied().collect();
            let mut internal = 0i64;
            let mut incident = 0i64;
            for id in &members {
                if let Some(neighbors) = adj.get(id) {
                    for (other, w) in neighbors {
                        incident += *w;
                        if member_set.contains(other) {
                            internal += *w;
                        }
                    }
                }
            }
            json!({
                "id": cid,
                "size": members.len(),
                "cohesion": if incident == 0 { 0.0 } else { (internal as f64 / incident as f64 * 1000.0).round() / 1000.0 },
                "members": members_qn.into_iter().take(20).collect::<Vec<_>>(),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b["size"]
            .as_i64()
            .cmp(&a["size"].as_i64())
            .then(a["id"].as_i64().cmp(&b["id"].as_i64()))
    });
    json!({
        "communities": rows.into_iter().take(15).collect::<Vec<_>>(),
        "modularity": (modularity * 1000.0).round() / 1000.0,
        "algorithm": "louvain-lite",
    })
}

fn cycles(nodes: &[&Node], edges: &[&Edge], by_id: &HashMap<i64, &Node>) -> Value {
    let mut adj: HashMap<i64, Vec<i64>> = HashMap::new();
    for edge in edges {
        if edge.edge_type == "CALLS" {
            if let (Some(s), Some(t)) = (by_id.get(&edge.source_id), by_id.get(&edge.target_id)) {
                if labels::is_callable(&s.label) && labels::is_callable(&t.label) {
                    adj.entry(s.id).or_default().push(t.id);
                }
            }
        }
    }
    let mut index = 0i64;
    let mut indices: HashMap<i64, i64> = HashMap::new();
    let mut lowlink: HashMap<i64, i64> = HashMap::new();
    let mut stack: Vec<i64> = Vec::new();
    let mut on_stack: HashSet<i64> = HashSet::new();
    let mut components: Vec<Vec<i64>> = Vec::new();
    let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
    for id in ids {
        if !indices.contains_key(&id) {
            strong_connect(
                id,
                &adj,
                &mut index,
                &mut indices,
                &mut lowlink,
                &mut stack,
                &mut on_stack,
                &mut components,
            );
        }
    }
    let mut rows: Vec<Value> = components
        .into_iter()
        .filter(|c| c.len() > 1)
        .map(|members| {
            let mut qns: Vec<Value> = members
                .iter()
                .filter_map(|id| by_id.get(id))
                .map(|n| json!({"qn": n.qualified_name, "name": n.name, "file": n.file_path}))
                .collect();
            qns.sort_by(|a, b| a["qn"].as_str().cmp(&b["qn"].as_str()));
            json!({"size": members.len(), "members": qns.into_iter().take(20).collect::<Vec<_>>()})
        })
        .collect();
    rows.sort_by(|a, b| {
        b["size"].as_i64().cmp(&a["size"].as_i64()).then(
            a["members"][0]["qn"]
                .as_str()
                .cmp(&b["members"][0]["qn"].as_str()),
        )
    });
    json!({
        "cycles": rows.into_iter().take(50).collect::<Vec<_>>(),
        "note": "Strongly connected components over CALLS; a cycle is not necessarily a defect.",
    })
}

#[allow(clippy::too_many_arguments)]
fn strong_connect(
    v: i64,
    adj: &HashMap<i64, Vec<i64>>,
    index: &mut i64,
    indices: &mut HashMap<i64, i64>,
    lowlink: &mut HashMap<i64, i64>,
    stack: &mut Vec<i64>,
    on_stack: &mut HashSet<i64>,
    components: &mut Vec<Vec<i64>>,
) {
    indices.insert(v, *index);
    lowlink.insert(v, *index);
    *index += 1;
    stack.push(v);
    on_stack.insert(v);
    if let Some(neighbors) = adj.get(&v) {
        for w in neighbors {
            if !indices.contains_key(w) {
                strong_connect(
                    *w, adj, index, indices, lowlink, stack, on_stack, components,
                );
                let low = lowlink[&v].min(lowlink[w]);
                lowlink.insert(v, low);
            } else if on_stack.contains(w) {
                let low = lowlink[&v].min(*indices.get(w).unwrap_or(&i64::MAX));
                lowlink.insert(v, low);
            }
        }
    }
    if lowlink.get(&v) == indices.get(&v) {
        let mut component = Vec::new();
        while let Some(w) = stack.pop() {
            on_stack.remove(&w);
            component.push(w);
            if w == v {
                break;
            }
        }
        components.push(component);
    }
}
