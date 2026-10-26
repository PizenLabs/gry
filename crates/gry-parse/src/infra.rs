//! Infrastructure extraction: containers, orchestration, provisioning, CI and
//! deployment commands. Relationships are emitted as `InfraNode`/`InfraEdge`
//! records with deterministic qualified names and are attached to the graph by
//! the indexing pipeline.

use crate::model::*;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub const FILE_SELF: &str = "@self";
pub const FILE_PREFIX: &str = "@file:";

pub fn parse_infra(rel_path: &str, lang: &str, source: &[u8], out: &mut FileExtract) {
    match lang {
        "dockerfile" => parse_dockerfile(rel_path, source, out),
        "make" => parse_makefile(rel_path, source, out),
        "compose" => parse_compose(rel_path, source, out),
        "kubernetes" => parse_kubernetes(rel_path, source, out, None),
        "kustomize" => parse_kustomize(rel_path, source, out),
        "helm" => parse_helm(rel_path, source, out),
        "github_actions" => parse_ci(rel_path, source, out),
        "ansible" => parse_ansible(rel_path, source, out),
        "hcl" => parse_terraform(rel_path, source, out),
        "sql" => parse_dbt(rel_path, source, out),
        "yaml" => {
            let text = String::from_utf8_lossy(source);
            if text.contains("{{") && rel_path.contains("templates/") {
                parse_helm_template(rel_path, source, out);
            } else if rel_path.ends_with("values.yaml") || rel_path.ends_with("values.yml") {
                parse_helm_values(rel_path, source, out);
            } else if text.contains("apiVersion:") {
                if rel_path.contains("templates/")
                    && (rel_path.contains("/templates/") || rel_path.contains("charts/"))
                {
                    parse_kubernetes(rel_path, source, out, Some("helm"));
                } else {
                    parse_kubernetes(rel_path, source, out, None);
                }
            } else if text.contains("on:") && text.contains("jobs:") {
                parse_ci(rel_path, source, out);
            }
        }
        _ => {}
    }
}

fn add_node(
    out: &mut FileExtract,
    rel_path: &str,
    label: &str,
    name: &str,
    qn: &str,
    line: i32,
    properties: Map<String, Value>,
) {
    let mut props = properties;
    props.insert("ecosystem".to_string(), Value::String("infra".to_string()));
    out.infra_nodes.push(InfraNode {
        label: label.to_string(),
        name: name.to_string(),
        qualified_name: qn.to_string(),
        file_path: rel_path.to_string(),
        start_line: line,
        end_line: line,
        properties: props,
    });
}

fn add_edge(
    out: &mut FileExtract,
    source: &str,
    target: &str,
    edge_type: &str,
    props: Map<String, Value>,
) {
    out.infra_edges.push(InfraEdge {
        source_qn: source.to_string(),
        target_qn: target.to_string(),
        edge_type: edge_type.to_string(),
        properties: props,
    });
}

fn props(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
        .collect()
}

// ── Dockerfile ──────────────────────────────────────────────────────

static DOCKER_FROM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^FROM\s+(?:--platform=\S+\s+)?(\S+)(?:\s+AS\s+(\S+))?").unwrap());
static DOCKER_ENV: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)^ENV\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s*=\s*|\s+)(.*)$"#).unwrap());
static DOCKER_RUN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(?:ENTRYPOINT|CMD)\s+(.*)$").unwrap());

fn parse_dockerfile(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<String> = join_continuations(&text);
    let mut stages: HashMap<String, String> = HashMap::new();
    let mut last_stage = String::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(cap) = DOCKER_FROM.captures(trimmed) {
            let image_ref = cap.get(1).unwrap().as_str().to_string();
            let (repo, tag, digest) = parse_image_ref(&image_ref);
            // A stage name is not an image.
            if image_ref != "scratch" {
                let qn = image_qn(&repo);
                let mut p = props(&[("repository", &repo), ("registry", "unknown")]);
                if let Some(tag) = &tag {
                    p.insert("tag".to_string(), Value::String(tag.clone()));
                }
                if let Some(digest) = &digest {
                    p.insert("digest".to_string(), Value::String(digest.clone()));
                }
                add_node(out, rel_path, "Image", &image_ref, &qn, idx as i32 + 1, p);
                let edge_props = props(&[("via", "from")]);
                if let Some(stage) = cap.get(2) {
                    stages.insert(stage.as_str().to_ascii_lowercase(), qn.clone());
                }
                add_edge(out, FILE_SELF, &qn, "DEPENDS_ON", edge_props);
            }
            if let Some(stage) = cap.get(2) {
                stages.insert(stage.as_str().to_ascii_lowercase(), String::new());
                last_stage = stage.as_str().to_string();
            }
        }
        if let Some(cap) = DOCKER_ENV.captures(trimmed) {
            let key = cap.get(1).unwrap().as_str();
            let qn = env_qn(key);
            add_node(
                out,
                rel_path,
                "EnvVar",
                key,
                &qn,
                idx as i32 + 1,
                props(&[("strategy", "env_set"), ("source", "dockerfile")]),
            );
            add_edge(
                out,
                FILE_SELF,
                &qn,
                "CONFIGURES",
                props(&[("strategy", "env_set"), ("source", "dockerfile")]),
            );
        }
        if let Some(cap) = DOCKER_RUN.captures(trimmed) {
            let cmd = cap.get(1).unwrap().as_str().trim();
            let program = command_program(cmd);
            if let Some(program) = program {
                let qn = process_qn(&program);
                add_node(
                    out,
                    rel_path,
                    "Process",
                    &program,
                    &qn,
                    idx as i32 + 1,
                    props(&[("dynamic", "false")]),
                );
                add_edge(
                    out,
                    FILE_SELF,
                    &qn,
                    "SPAWNS",
                    props(&[("via", "dockerfile")]),
                );
            }
        }
        if let Some(rest) = trimmed.strip_prefix("COPY ") {
            for part in rest.split_whitespace() {
                if part.starts_with("--from=") {
                    let stage = part.trim_start_matches("--from=");
                    if let Some(qn) = stages.get(&stage.to_ascii_lowercase()) {
                        if !qn.is_empty() {
                            add_edge(
                                out,
                                FILE_SELF,
                                qn,
                                "DEPENDS_ON",
                                props(&[("via", "copy-from")]),
                            );
                        }
                    }
                }
            }
        }
    }
    let _ = last_stage;
}

fn join_continuations(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let trimmed_end = line.trim_end();
        if let Some(stripped) = trimmed_end.strip_suffix('\\') {
            current.push_str(stripped);
            current.push(' ');
        } else {
            current.push_str(line);
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn parse_image_ref(raw: &str) -> (String, Option<String>, Option<String>) {
    let raw = raw.trim();
    let (without_digest, digest) = match raw.split_once('@') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (raw, None),
    };
    // Do not invent docker.io: repository is as written, registry "unknown"
    // unless the first segment looks like a host.
    let (repo, tag) = match without_digest.rsplit_once(':') {
        Some((r, t))
            if !r.contains('/') || r.split('/').next().is_some_and(|s| s.contains('.')) =>
        {
            (r.to_string(), Some(t.to_string()))
        }
        Some((r, t)) if !t.contains('/') => (r.to_string(), Some(t.to_string())),
        _ => (without_digest.to_string(), None),
    };
    (repo, tag, digest)
}

fn image_qn(repo: &str) -> String {
    format!("__image__{repo}")
}

fn env_qn(key: &str) -> String {
    format!("__env__{key}")
}

fn process_qn(program: &str) -> String {
    format!("__process__{program}")
}

pub fn route_qn(method: &str, path: &str) -> String {
    format!("__route__{}__{}", method.to_ascii_uppercase(), path)
}

pub fn channel_qn(name: &str) -> String {
    format!("__channel__{name}")
}

// ── Compose ─────────────────────────────────────────────────────────

fn parse_compose(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let value: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(services) = value.get("services").and_then(Value::as_object) else {
        return;
    };
    let service_qn = |name: &str| format!("{rel_path}::compose/Service/-/{name}");
    for (name, service) in services {
        let mut p = props(&[
            ("kind", "Service"),
            ("ecosystem", "compose"),
            ("obj_key", name),
        ]);
        if let Some(image) = service.get("image").and_then(Value::as_str) {
            p.insert("image".to_string(), Value::String(image.to_string()));
        }
        add_node(out, rel_path, "Resource", name, &service_qn(name), 0, p);
        if let Some(image) = service.get("image").and_then(Value::as_str) {
            let (repo, tag, digest) = parse_image_ref(image);
            let qn = image_qn(&repo);
            let mut ip = props(&[("repository", &repo), ("registry", "unknown")]);
            if let Some(tag) = tag {
                ip.insert("tag".to_string(), Value::String(tag));
            }
            if let Some(digest) = digest {
                ip.insert("digest".to_string(), Value::String(digest));
            }
            add_node(out, rel_path, "Image", image, &qn, 0, ip);
            add_edge(
                out,
                &service_qn(name),
                &qn,
                "USAGE",
                props(&[("ref", "image")]),
            );
        }
        if let Some(depends) = service.get("depends_on") {
            let names: Vec<String> = match depends {
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                Value::Object(map) => map.keys().cloned().collect(),
                _ => Vec::new(),
            };
            for dep in names {
                add_edge(
                    out,
                    &service_qn(name),
                    &service_qn(&dep),
                    "DEPENDS_ON",
                    props(&[("via", "compose")]),
                );
            }
        }
        if let Some(env) = service.get("environment") {
            let entries: Vec<(String, String)> = match env {
                Value::Object(map) => map
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect(),
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|s| s.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            for (key, value) in entries {
                let eqn = env_qn(&key);
                add_node(
                    out,
                    rel_path,
                    "EnvVar",
                    &key,
                    &eqn,
                    0,
                    props(&[("strategy", "env_set"), ("source", "compose")]),
                );
                add_edge(
                    out,
                    &service_qn(name),
                    &eqn,
                    "CONFIGURES",
                    props(&[("strategy", "env_set"), ("source", "compose")]),
                );
                // Compose service addresses: another service name or host in a URL.
                if let Some(target) = compose_target(&value, services) {
                    add_edge(
                        out,
                        &service_qn(name),
                        &service_qn(&target),
                        "CONNECTS_TO",
                        props(&[("via", "env"), ("key", &key), ("container", name)]),
                    );
                }
            }
        }
    }
}

fn compose_target(value: &str, services: &serde_json::Map<String, Value>) -> Option<String> {
    let host = if let Some(rest) = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
    {
        rest.split(['/', ':']).next().unwrap_or("")
    } else {
        value.split(':').next().unwrap_or("")
    };
    services.keys().find(|s| s.as_str() == host).cloned()
}

// ── Kubernetes / Helm ───────────────────────────────────────────────

/// Workload candidate for service-selector matching:
/// `(qualified_name, kind, selector_labels, pod_labels)`.
type K8sResource = (String, String, Vec<(String, String)>, Vec<(String, String)>);

fn parse_kubernetes(rel_path: &str, source: &[u8], out: &mut FileExtract, ecosystem: Option<&str>) {
    let text = String::from_utf8_lossy(source);
    let mut docs = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.trim() == "---" {
            if !current.trim().is_empty() {
                docs.push(std::mem::take(&mut current));
            }
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        docs.push(current);
    }
    if docs.is_empty() {
        docs.push(text.to_string());
    }
    let mut resources: Vec<K8sResource> = Vec::new();
    for doc in docs {
        let value: Value = match serde_yaml::from_str(&doc) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("apiVersion").is_none() || value.get("kind").is_none() {
            continue;
        }
        let api_version = value
            .get("apiVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
        let (group, version) = match api_version.split_once('/') {
            Some((g, v)) => (g, v),
            None => ("", api_version),
        };
        let meta = value.get("metadata").cloned().unwrap_or(Value::Null);
        let name = meta.get("name").and_then(Value::as_str).unwrap_or("");
        let namespace = meta.get("namespace").and_then(Value::as_str).unwrap_or("-");
        if name.is_empty() || kind.is_empty() {
            continue;
        }
        let qn = format!("{rel_path}::{group}/{kind}/{namespace}/{name}");
        let labels = labels_string(meta.get("labels"));
        let mut p = props(&[
            ("kind", kind),
            ("version", version),
            ("obj_key", &format!("{group}/{kind}/{namespace}/{name}")),
            ("ecosystem", ecosystem.unwrap_or("kubernetes")),
        ]);
        if group != "v1" && !group.is_empty() {
            p.insert("group".to_string(), Value::String(group.to_string()));
        }
        if namespace != "-" {
            p.insert(
                "namespace".to_string(),
                Value::String(namespace.to_string()),
            );
        }
        if !labels.is_empty() {
            p.insert("labels".to_string(), Value::String(labels.clone()));
        }
        add_node(out, rel_path, "Resource", name, &qn, 0, p);

        // Images anywhere in the object.
        let mut images = Vec::new();
        collect_key_values(&value, "image", &mut images);
        for image in images {
            let (repo, tag, digest) = parse_image_ref(&image);
            let iqn = image_qn(&repo);
            let mut ip = props(&[("repository", &repo), ("registry", "unknown")]);
            if let Some(tag) = tag {
                ip.insert("tag".to_string(), Value::String(tag));
            }
            if let Some(digest) = digest {
                ip.insert("digest".to_string(), Value::String(digest));
            }
            add_node(out, rel_path, "Image", &image, &iqn, 0, ip);
            add_edge(out, &qn, &iqn, "USAGE", props(&[("ref", "image")]));
        }
        // Container env: set + potential service connection.
        let mut envs = Vec::new();
        collect_env(&value, &mut envs);
        for (key, value_text) in envs {
            let eqn = env_qn(&key);
            add_node(
                out,
                rel_path,
                "EnvVar",
                &key,
                &eqn,
                0,
                props(&[("strategy", "env_set"), ("source", "kubernetes")]),
            );
            add_edge(
                out,
                &qn,
                &eqn,
                "CONFIGURES",
                props(&[("strategy", "env_set"), ("source", "kubernetes")]),
            );
            if let Some(host) = host_from_env_value(&value_text) {
                add_edge(
                    out,
                    &qn,
                    &format!("__k8s_svc__{namespace}/{host}"),
                    "CONNECTS_TO",
                    props(&[("via", "env"), ("key", &key)]),
                );
            }
        }
        let selector = value
            .pointer("/spec/selector")
            .map(|v| labels_string(Some(v)))
            .unwrap_or_default();
        let pod_labels = value
            .pointer("/spec/template/metadata/labels")
            .or_else(|| value.pointer("/spec/jobTemplate/spec/template/metadata/labels"))
            .map(|v| labels_string(Some(v)))
            .unwrap_or_default();
        if !selector.is_empty() || !pod_labels.is_empty() {
            resources.push((
                qn.clone(),
                kind.to_string(),
                parse_label_pairs(&selector),
                parse_label_pairs(&pod_labels),
            ));
        }
        if let Some(ecosystem) = ecosystem {
            if ecosystem == "helm" {
                // Chart DEPLOYS the rendered object; target resolved by pipeline.
                add_edge(out, "@chart", &qn, "DEPLOYS", props(&[("via", "helm")]));
            }
        }
    }
    // Service → workload label-selector matching (same document bundle).
    for (svc_qn, svc_kind, selector, _) in &resources {
        if svc_kind != "Service" || selector.is_empty() {
            continue;
        }
        for (work_qn, work_kind, _, pod_labels) in &resources {
            if work_qn == svc_qn || work_kind == "Service" {
                continue;
            }
            if !pod_labels.is_empty()
                && selector
                    .iter()
                    .all(|(k, v)| pod_labels.iter().any(|(pk, pv)| pk == k && pv == v))
            {
                add_edge(
                    out,
                    svc_qn,
                    work_qn,
                    "INFRA_MAPS",
                    props(&[("via", "selector")]),
                );
            }
        }
    }
}

fn labels_string(labels: Option<&Value>) -> String {
    let Some(Value::Object(map)) = labels else {
        return String::new();
    };
    let mut pairs: Vec<String> = map
        .iter()
        .filter_map(|(k, v)| Some(format!("{k}={}", v.as_str()?)))
        .collect();
    pairs.sort();
    pairs.join(",")
}

fn parse_label_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn collect_key_values(value: &Value, key: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == key {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                }
                collect_key_values(v, key, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_key_values(item, key, out);
            }
        }
        _ => {}
    }
}

fn collect_env(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let Some(env) = map.get("env").and_then(Value::as_array) {
                for item in env {
                    let name = item.get("name").and_then(Value::as_str);
                    let value = item.get("value").and_then(Value::as_str);
                    if let (Some(name), Some(value)) = (name, value) {
                        out.push((name.to_string(), value.to_string()));
                    }
                }
            }
            for v in map.values() {
                collect_env(v, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_env(item, out);
            }
        }
        _ => {}
    }
}

fn host_from_env_value(value: &str) -> Option<String> {
    let host = if let Some(rest) = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
    {
        rest.split(['/', ':']).next()?
    } else {
        let candidate = value.split(':').next()?;
        if candidate.contains('.') || value.contains("://") {
            candidate
        } else {
            return None;
        }
    };
    if host.is_empty() || host == "localhost" {
        None
    } else {
        Some(host.to_string())
    }
}

// ── Kustomize ───────────────────────────────────────────────────────

fn parse_kustomize(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let value: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    for key in [
        "resources",
        "bases",
        "components",
        "patches",
        "patchesStrategicMerge",
    ] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                let path = item
                    .as_str()
                    .or_else(|| item.get("path").and_then(Value::as_str));
                if let Some(path) = path {
                    add_edge(
                        out,
                        FILE_SELF,
                        &format!("{FILE_PREFIX}{}", normalize_rel(rel_path, path)),
                        "IMPORTS",
                        props(&[("local_name", key)]),
                    );
                }
            }
        }
    }
    let dir = rel_path.rsplit_once('/').map(|(d, _)| d).unwrap_or(".");
    for (key, label) in [
        ("namePrefix", "namePrefix"),
        ("nameSuffix", "nameSuffix"),
        ("namespace", "namespace"),
    ] {
        if let Some(value) = value.get(key).and_then(Value::as_str) {
            let qn = format!("__kustomize__{dir}.{key}");
            add_node(
                out,
                rel_path,
                "Variable",
                value,
                &qn,
                0,
                props(&[("ecosystem", "kustomize"), ("key", label)]),
            );
            add_edge(
                out,
                FILE_SELF,
                &qn,
                "CONFIGURES",
                props(&[("via", "kustomize"), ("key", label)]),
            );
        }
    }
    if let Some(images) = value.get("images").and_then(Value::as_array) {
        for image in images {
            let Some(base) = image.get("name").and_then(Value::as_str) else {
                continue;
            };
            let new_name = image.get("newName").and_then(Value::as_str).unwrap_or(base);
            let new_tag = image.get("newTag").and_then(Value::as_str).unwrap_or("");
            let rendered = if new_tag.is_empty() {
                new_name.to_string()
            } else {
                format!("{new_name}:{new_tag}")
            };
            let base_qn = format!("__image__{base}");
            let new_qn = format!("__image__{rendered}");
            add_node(
                out,
                rel_path,
                "Image",
                &rendered,
                &new_qn,
                0,
                props(&[("ecosystem", "kustomize"), ("override_of", base)]),
            );
            add_edge(
                out,
                FILE_SELF,
                &new_qn,
                "OVERRIDE",
                props(&[("via", "kustomize"), ("base", base)]),
            );
            let _ = base_qn;
        }
    }
    if let Some(charts) = value.get("helmCharts").and_then(Value::as_array) {
        for chart in charts {
            if let Some(name) = chart.get("name").and_then(Value::as_str) {
                let qn = format!("__chart__{name}");
                add_node(
                    out,
                    rel_path,
                    "Chart",
                    name,
                    &qn,
                    0,
                    props(&[("via", "helmcharts")]),
                );
                add_edge(
                    out,
                    FILE_SELF,
                    &qn,
                    "DEPLOYS",
                    props(&[("via", "helmcharts")]),
                );
            }
        }
    }
}

// ── Helm ────────────────────────────────────────────────────────────

fn parse_helm(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let value: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let name = value.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return;
    }
    let qn = format!("__chart__{name}");
    let mut p = props(&[
        (
            "version",
            value.get("version").and_then(Value::as_str).unwrap_or(""),
        ),
        (
            "apiVersion",
            value
                .get("apiVersion")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
    ]);
    if let Some(desc) = value.get("description").and_then(Value::as_str) {
        p.insert("docstring".to_string(), Value::String(desc.to_string()));
    }
    add_node(out, rel_path, "Chart", name, &qn, 0, p);
    if let Some(deps) = value.get("dependencies").and_then(Value::as_array) {
        for dep in deps {
            if let Some(dep_name) = dep.get("name").and_then(Value::as_str) {
                let dep_qn = format!("__chart__{dep_name}");
                add_node(
                    out,
                    rel_path,
                    "Chart",
                    dep_name,
                    &dep_qn,
                    0,
                    props(&[("via", "dependency")]),
                );
                add_edge(
                    out,
                    &qn,
                    &dep_qn,
                    "DEPENDS_ON",
                    props(&[("via", "dependency")]),
                );
            }
        }
    }
}

fn helm_chart_dir(rel_path: &str) -> String {
    if let Some((dir, _)) = rel_path.rsplit_once("/templates/") {
        dir.to_string()
    } else {
        rel_path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default()
    }
}

fn parse_helm_template(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let chart_dir = helm_chart_dir(rel_path);
    let mut kind = String::new();
    let mut resource_name = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("kind:") {
            kind = rest.trim().to_string();
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            if resource_name.is_empty() {
                resource_name = rest.trim().trim_matches(['\'', '"']).to_string();
            }
        }
        if let Some(cap) = HELM_IMAGE.captures(line) {
            let image = cap.get(1).unwrap().as_str();
            let qn = format!("__image__{image}");
            let mut p = props(&[("ecosystem", "helm"), ("template", "true")]);
            if !chart_dir.is_empty() {
                p.insert("chart".to_string(), Value::String(chart_dir.clone()));
            }
            add_node(out, rel_path, "Image", image, &qn, 0, p);
            add_edge(out, FILE_SELF, &qn, "DEPLOYS", props(&[("via", "helm")]));
        }
        for cap in HELM_VALUE.captures_iter(line) {
            let key = cap.get(1).unwrap().as_str();
            let qn = format!("__values__{chart_dir}.{key}");
            add_node(
                out,
                rel_path,
                "Variable",
                key,
                &qn,
                0,
                props(&[("ecosystem", "helm"), ("key", key)]),
            );
            add_edge(
                out,
                FILE_SELF,
                &qn,
                "CONFIGURES",
                props(&[("via", "helm"), ("key", key)]),
            );
        }
    }
    if !kind.is_empty() {
        let name = if resource_name.is_empty() {
            rel_path
                .rsplit('/')
                .next()
                .unwrap_or(rel_path)
                .trim_end_matches(".yaml")
                .to_string()
        } else {
            resource_name
        };
        let qn = format!("__k8s_res__{}/{}/{}", chart_dir, kind, name);
        add_node(
            out,
            rel_path,
            "Resource",
            &name,
            &qn,
            0,
            props(&[("kind", &kind), ("ecosystem", "helm"), ("template", "true")]),
        );
        add_edge(out, FILE_SELF, &qn, "DEFINES", props(&[("via", "helm")]));
    }
}

fn parse_helm_values(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let Ok(value) = serde_yaml::from_str::<Value>(&text) else {
        return;
    };
    let chart_dir = rel_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    fn walk(value: &Value, prefix: &str, chart_dir: &str, rel_path: &str, out: &mut FileExtract) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(child, &path, chart_dir, rel_path, out);
                }
            }
            Value::Null | Value::Array(_) => {}
            scalar => {
                let qn = format!("__values__{chart_dir}.{prefix}");
                let rendered = scalar.to_string();
                add_node(
                    out,
                    rel_path,
                    "Variable",
                    prefix,
                    &qn,
                    0,
                    props(&[
                        ("ecosystem", "helm"),
                        ("key", prefix),
                        ("default", &rendered),
                    ]),
                );
                add_edge(
                    out,
                    FILE_SELF,
                    &qn,
                    "DEFINES",
                    props(&[("via", "helm-values")]),
                );
            }
        }
    }
    walk(&value, "", &chart_dir, rel_path, out);
}

// ── CI ──────────────────────────────────────────────────────────────

fn parse_ci(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let value: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(jobs) = value.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job, body) in jobs {
        let qn = format!("{rel_path}::ci/{job}");
        add_node(
            out,
            rel_path,
            "Function",
            job,
            &qn,
            0,
            props(&[("ci", "true"), ("job", job)]),
        );
        add_edge(out, FILE_SELF, &qn, "DEFINES", props(&[("ci", "true")]));
        if let Some(steps) = body.get("steps").and_then(Value::as_array) {
            for step in steps {
                if let Some(run) = step.get("run").and_then(Value::as_str) {
                    for command in run.lines() {
                        record_command(out, rel_path, &qn, command.trim(), Some(job));
                    }
                }
            }
        }
    }
}

// ── Make ────────────────────────────────────────────────────────────

static MAKE_TARGET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^([A-Za-z0-9_][A-Za-z0-9_.\-/]*)\s*:").unwrap());

/// Shell globs are not file paths. `go test ./...` must not become a
/// reference to a file literally named `...`.
fn is_glob_token(token: &str) -> bool {
    token.contains("...") || token.contains('*') || token.contains('?') || token.contains('[')
}

fn parse_makefile(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let mut current: Option<(String, i32)> = None;
    for (idx, line) in text.lines().enumerate() {
        if line.starts_with('\t') {
            if let Some((qn, _)) = &current {
                record_command(out, rel_path, qn, line.trim(), None);
            }
            continue;
        }
        if let Some(cap) = MAKE_TARGET.captures(line) {
            let target = cap.get(1).unwrap().as_str();
            if target.contains('%')
                || target.starts_with('.')
                || line[cap.get(0).unwrap().end()..].starts_with('=')
            {
                current = None;
                continue;
            }
            let qn = format!("{rel_path}::target/{target}");
            add_node(
                out,
                rel_path,
                "Function",
                target,
                &qn,
                idx as i32 + 1,
                props(&[("kind", "make-target")]),
            );
            add_edge(
                out,
                FILE_SELF,
                &qn,
                "DEFINES",
                props(&[("kind", "make-target")]),
            );
            current = Some((qn, idx as i32 + 1));
        } else if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            current = None;
        }
    }
}

fn record_command(
    out: &mut FileExtract,
    rel_path: &str,
    source_qn: &str,
    command: &str,
    job: Option<&str>,
) {
    let command = command.trim();
    if command.is_empty() || command.starts_with('#') {
        return;
    }
    // Scripts, programs, deploys and HTTP calls.
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return;
    }
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        if is_glob_token(token) {
            i += 1;
            continue;
        }
        if matches!(token, "kubectl" | "helm" | "kustomize" | "terraform") {
            if let Some((flag, value)) =
                find_flag(&tokens[i..], &["-f", "--filename", "-k", "--kustomize"])
            {
                let path = normalize_rel(rel_path, value);
                add_edge(
                    out,
                    source_qn,
                    &format!("{FILE_PREFIX}{path}"),
                    "DEPLOYS",
                    props(&[("via", flag), ("ci", job.unwrap_or(""))]),
                );
            }
        }
        if matches!(token, "docker" | "podman" | "nerdctl" | "buildah")
            && tokens.get(i + 1) == Some(&"build")
        {
            if let Some((_, tag)) = find_flag(&tokens[i..], &["-t", "--tag"]) {
                let (repo, tag, digest) = parse_image_ref(tag);
                let qn = image_qn(&repo);
                let mut p = props(&[("repository", &repo), ("registry", "unknown")]);
                if let Some(tag_value) = &tag {
                    p.insert("tag".to_string(), Value::String(tag_value.clone()));
                }
                if let Some(digest) = digest {
                    p.insert("digest".to_string(), Value::String(digest));
                }
                let display = tag.clone().unwrap_or_else(|| repo.clone());
                add_node(out, rel_path, "Image", &display, &qn, 0, p);
                add_edge(
                    out,
                    source_qn,
                    &qn,
                    "DEPLOYS",
                    props(&[("via", "docker-build")]),
                );
            }
        }
        if matches!(token, "curl" | "wget") {
            if let Some(url) = tokens[i + 1..].iter().find(|t| t.starts_with("http")) {
                let (host, path) = split_url(url);
                let method = if tokens.iter().any(|t| matches!(*t, "-X" | "--request")) {
                    tokens
                        .iter()
                        .position(|t| matches!(*t, "-X" | "--request"))
                        .and_then(|idx| tokens.get(idx + 1))
                        .map(|m| m.to_ascii_uppercase())
                        .unwrap_or_else(|| "GET".to_string())
                } else if command.contains(" -d ") || command.contains("--data") {
                    "POST".to_string()
                } else {
                    "GET".to_string()
                };
                add_http_route(out, source_qn, &method, &path, host.as_deref());
            }
        }
        if token.starts_with("./")
            || token.ends_with(".sh")
            || matches!(
                token,
                "bash" | "sh" | "python" | "python3" | "node" | "ruby" | "go" | "cargo" | "make"
            )
        {
            let program = if matches!(
                token,
                "bash" | "sh" | "python" | "python3" | "node" | "ruby" | "go" | "cargo" | "make"
            ) {
                tokens.get(i + 1).copied().unwrap_or(token)
            } else {
                token
            };
            if is_glob_token(program) {
                i += 1;
                continue;
            }
            if program.starts_with("./") || program.ends_with(".sh") {
                add_edge(
                    out,
                    source_qn,
                    &format!("{FILE_PREFIX}{}", normalize_rel(rel_path, program)),
                    "SPAWNS",
                    props(&[("via", "shell"), ("ci", job.unwrap_or(""))]),
                );
            } else if !matches!(program, "build" | "test" | "run") {
                let qn = process_qn(program);
                add_node(
                    out,
                    rel_path,
                    "Process",
                    program,
                    &qn,
                    0,
                    props(&[("dynamic", "false")]),
                );
                add_edge(out, source_qn, &qn, "SPAWNS", props(&[("via", "shell")]));
            }
        }
        i += 1;
    }
}

fn find_flag<'a>(tokens: &'a [&'a str], flags: &[&str]) -> Option<(&'a str, &'a str)> {
    for (idx, token) in tokens.iter().enumerate() {
        if flags.contains(token) {
            if let Some(value) = tokens.get(idx + 1) {
                return Some((token, value));
            }
        }
        if let Some((flag, value)) = token.split_once('=') {
            if flags.contains(&flag) {
                return Some((flag, value));
            }
        }
    }
    None
}

fn add_http_route(
    out: &mut FileExtract,
    source_qn: &str,
    method: &str,
    path: &str,
    host: Option<&str>,
) {
    let qn = route_qn(method, path);
    let mut p = props(&[("method", method), ("url_path", path)]);
    if let Some(host) = host {
        p.insert("host".to_string(), Value::String(host.to_string()));
    }
    add_node(out, "", "Route", path, &qn, 0, p);
    add_edge(
        out,
        source_qn,
        &qn,
        "HTTP_CALLS",
        props(&[("method", method), ("url_path", path)]),
    );
}

fn split_url(url: &str) -> (Option<String>, String) {
    if let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    {
        let mut parts = rest.splitn(2, '/');
        let host = parts.next().unwrap_or("").to_string();
        let path = format!("/{}", parts.next().unwrap_or(""));
        (Some(host), path)
    } else {
        (None, url.to_string())
    }
}

fn command_program(command: &str) -> Option<String> {
    let cleaned = command.trim_start_matches(['[', '"', '\'']);
    let first = cleaned.split_whitespace().next()?;
    let first = first.trim_matches(['"', '\'', ']', ',']);
    if first.is_empty() || first.starts_with('#') {
        None
    } else {
        Some(first.to_string())
    }
}

// ── Ansible ─────────────────────────────────────────────────────────

fn parse_ansible(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let value: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Value::Array(plays) = value else {
        return;
    };
    let mut handlers: HashSet<String> = HashSet::new();
    for play in &plays {
        if let Some(handler_list) = play.get("handlers").and_then(Value::as_array) {
            for h in handler_list {
                if let Some(name) = h.get("name").and_then(Value::as_str) {
                    handlers.insert(name.to_string());
                }
            }
        }
    }
    for (pi, play) in plays.iter().enumerate() {
        if let Some(roles) = play.get("roles") {
            let names: Vec<String> = match roles {
                Value::Array(items) => items
                    .iter()
                    .filter_map(|i| {
                        i.as_str()
                            .map(str::to_string)
                            .or_else(|| i.get("role").and_then(Value::as_str).map(str::to_string))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for role in names {
                add_edge(
                    out,
                    FILE_SELF,
                    &format!("{FILE_PREFIX}roles/{role}"),
                    "IMPORTS",
                    props(&[("local_name", &role), ("via", "ansible")]),
                );
            }
        }
        for key in [
            "import_playbook",
            "include_tasks",
            "import_tasks",
            "include",
        ] {
            if let Some(path) = play.get(key).and_then(Value::as_str) {
                add_edge(
                    out,
                    FILE_SELF,
                    &format!("{FILE_PREFIX}{}", normalize_rel(rel_path, path)),
                    "IMPORTS",
                    props(&[("local_name", key), ("via", "ansible")]),
                );
            }
        }
        if let Some(tasks) = play.get("tasks").and_then(Value::as_array) {
            for (ti, task) in tasks.iter().enumerate() {
                let name = task
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("task-{pi}-{ti}"));
                let qn = format!("{rel_path}::task/{name}");
                add_node(
                    out,
                    rel_path,
                    "Function",
                    &name,
                    &qn,
                    0,
                    props(&[("source", "ansible")]),
                );
                add_edge(
                    out,
                    FILE_SELF,
                    &qn,
                    "DEFINES",
                    props(&[("source", "ansible")]),
                );
                for key in ["include_tasks", "import_tasks", "include"] {
                    if let Some(path) = task.get(key).and_then(Value::as_str) {
                        add_edge(
                            out,
                            &qn,
                            &format!("{FILE_PREFIX}{}", normalize_rel(rel_path, path)),
                            "IMPORTS",
                            props(&[("local_name", key), ("read", &name)]),
                        );
                    }
                }
                if let Some(notify) = task.get("notify") {
                    let targets: Vec<&str> = match notify {
                        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
                        Value::String(s) => vec![s.as_str()],
                        _ => Vec::new(),
                    };
                    for target in targets {
                        if handlers.contains(target) {
                            add_edge(
                                out,
                                &qn,
                                &format!("{rel_path}::task/{target}"),
                                "CALLS",
                                props(&[("via", "notify")]),
                            );
                        }
                    }
                }
                if let Some(template) = task.get("template").and_then(Value::as_str) {
                    let role = rel_path.split('/').nth(2).unwrap_or("");
                    let target = if role.is_empty() {
                        format!("{FILE_PREFIX}{}", normalize_rel(rel_path, template))
                    } else {
                        format!(
                            "{FILE_PREFIX}roles/{role}/templates/{}",
                            template.rsplit('/').next().unwrap_or(template)
                        )
                    };
                    add_edge(out, &qn, &target, "DEPLOYS", props(&[("via", "ansible")]));
                }
            }
        }
    }
}

// ── Terraform / OpenTofu ────────────────────────────────────────────

static HELM_IMAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"^\s*image:\s*['\"]?([^'\"\s]+)"#).unwrap());
static HELM_VALUE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\{\{\s*\.Values\.([A-Za-z0-9_.]+)\s*\}\}"#).unwrap());

static TF_BLOCK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^\s*(variable|resource|module|data|output|locals)\s+"?([A-Za-z0-9_\-]+)"?(?:\s+"([A-Za-z0-9_\-]+)")?"#).unwrap()
});
static TF_ASSIGN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"^\s*([A-Za-z0-9_\-]+)\s*=\s*"([^"]*)""#).unwrap());
static TF_BARE_DEFAULT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*default\s*=\s*([^\s#]+)").unwrap());
static TF_REF: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b(var|local|module|data|resource)\.([A-Za-z0-9_\-]+)").unwrap());

fn parse_terraform(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let is_tfvars = rel_path.ends_with(".tfvars");
    if is_tfvars {
        for (idx, line) in text.lines().enumerate() {
            if let Some(cap) = TF_ASSIGN.captures(line) {
                let name = cap.get(1).unwrap().as_str();
                let qn = format!("terraform.variable.{name}");
                add_node(
                    out,
                    rel_path,
                    "Variable",
                    name,
                    &qn,
                    idx as i32 + 1,
                    props(&[("ecosystem", "terraform"), ("ref", "tfvars")]),
                );
                add_edge(
                    out,
                    FILE_SELF,
                    &qn,
                    "OVERRIDE",
                    props(&[("via", "tfvars"), ("line", &(idx + 1).to_string())]),
                );
            }
        }
        return;
    }
    let mut stack: Vec<String> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('}') {
            stack.pop();
            continue;
        }
        if let Some(cap) = TF_BLOCK.captures(line) {
            let block_type = cap.get(1).unwrap().as_str();
            let name = cap.get(2).unwrap().as_str();
            let (label, qn) = match block_type {
                "variable" => ("Variable", format!("terraform.variable.{name}")),
                "resource" => ("Resource", format!("terraform.resource.{name}")),
                "data" => ("Resource", format!("terraform.data.{name}")),
                "module" => ("Module", format!("terraform.module.{name}")),
                "output" => ("Variable", format!("terraform.output.{name}")),
                "locals" => ("Module", format!("terraform.locals.{name}")),
                _ => continue,
            };
            let kind = cap.get(3).map(|m| m.as_str()).unwrap_or("");
            let p = props(&[
                ("ecosystem", "terraform"),
                ("ref", block_type),
                ("kind", kind),
            ]);
            if block_type == "resource" && name == "helm_release" && !kind.is_empty() {
                add_node(
                    out,
                    rel_path,
                    "Resource",
                    kind,
                    &qn,
                    idx as i32 + 1,
                    p.clone(),
                );
            } else {
                add_node(out, rel_path, label, name, &qn, idx as i32 + 1, p.clone());
            }
            if block_type == "variable" {
                let mut default = None;
                for follow in text.lines().skip(idx + 1).take(30) {
                    if follow.trim().starts_with('}') {
                        break;
                    }
                    if let Some(cap) = TF_ASSIGN.captures(follow) {
                        if cap.get(1).unwrap().as_str() == "default" {
                            default = Some(cap.get(2).unwrap().as_str().to_string());
                        }
                    } else if let Some(cap) = TF_BARE_DEFAULT.captures(follow) {
                        default = Some(cap.get(1).unwrap().as_str().to_string());
                    }
                }
                if let (Some(default), Some(node)) = (default, out.infra_nodes.last_mut()) {
                    if node.qualified_name == qn {
                        node.properties
                            .insert("default".to_string(), Value::String(default));
                    }
                }
            }
            stack.push(qn.clone());
            if block_type == "resource" && name == "helm_release" {
                // chart = "name" on a following line
                for follow in text.lines().skip(idx + 1).take(10) {
                    if let Some(cap) = TF_ASSIGN.captures(follow) {
                        if cap.get(1).unwrap().as_str() == "chart" {
                            let chart = cap.get(2).unwrap().as_str();
                            let cqn = format!("__chart__{chart}");
                            add_node(
                                out,
                                rel_path,
                                "Chart",
                                chart,
                                &cqn,
                                idx as i32 + 1,
                                props(&[("via", "terraform")]),
                            );
                            add_edge(out, &qn, &cqn, "DEPLOYS", props(&[("via", "terraform")]));
                        }
                    }
                }
            }
            continue;
        }
        if let Some((source_qn, _)) = stack.last().map(|q| (q.clone(), ())) {
            for cap in TF_REF.captures_iter(line) {
                let ref_kind = cap.get(1).unwrap().as_str();
                let ref_name = cap.get(2).unwrap().as_str();
                let target = match ref_kind {
                    "var" => format!("terraform.variable.{ref_name}"),
                    "local" => format!("terraform.locals.{ref_name}"),
                    "module" => format!("terraform.module.{ref_name}"),
                    "data" | "resource" => format!("terraform.resource.{ref_name}"),
                    _ => continue,
                };
                if target != source_qn {
                    add_edge(
                        out,
                        &source_qn,
                        &target,
                        "USAGE",
                        props(&[("via", "terraform"), ("ref", ref_kind)]),
                    );
                }
            }
            if let Some(cap) = TF_ASSIGN.captures(line) {
                let key = cap.get(1).unwrap().as_str();
                let value = cap.get(2).unwrap().as_str();
                if key == "source" && value.starts_with("./") {
                    add_edge(
                        out,
                        &source_qn,
                        &format!("{FILE_PREFIX}{}", normalize_rel(rel_path, value)),
                        "IMPORTS",
                        props(&[("local_name", "source"), ("via", "terraform")]),
                    );
                }
            }
        }
    }
}

// ── dbt ─────────────────────────────────────────────────────────────

fn parse_dbt(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    if !text.contains("{{") {
        return;
    }
    let ref_re = Regex::new(r#"\{\{\s*ref\s*\(\s*['\"]([^'\"]+)['\"]"#).unwrap();
    let source_re =
        Regex::new(r#"\{\{\s*source\s*\(\s*['\"]([^'\"]+)['\"]\s*,\s*['\"]([^'\"]+)['\"]"#)
            .unwrap();
    let mut deps: Vec<String> = Vec::new();
    for cap in ref_re.captures_iter(&text) {
        deps.push(cap.get(1).unwrap().as_str().to_string());
    }
    for cap in source_re.captures_iter(&text) {
        deps.push(format!(
            "{}.{}",
            cap.get(1).unwrap().as_str(),
            cap.get(2).unwrap().as_str()
        ));
    }
    deps.sort();
    deps.dedup();
    let model = rel_path
        .rsplit('/')
        .next()
        .unwrap_or(rel_path)
        .trim_end_matches(".sql")
        .to_string();
    let model_qn = format!("__table__{model}");
    add_node(
        out,
        rel_path,
        "Table",
        &model,
        &model_qn,
        0,
        props(&[("ecosystem", "dbt"), ("kind", "model")]),
    );
    add_edge(
        out,
        FILE_SELF,
        &model_qn,
        "DEFINES",
        props(&[("via", "dbt")]),
    );
    for dep in deps {
        let dep_qn = format!("__table__{dep}");
        add_node(
            out,
            rel_path,
            "Table",
            &dep,
            &dep_qn,
            0,
            props(&[("ecosystem", "dbt"), ("kind", "upstream")]),
        );
        add_edge(
            out,
            &model_qn,
            &dep_qn,
            "DEPENDS_ON",
            props(&[("via", "dbt")]),
        );
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn normalize_rel(from_file: &str, target: &str) -> String {
    if target.starts_with('/') {
        return target.trim_start_matches('/').to_string();
    }
    let mut stack: Vec<&str> = from_file
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').collect())
        .unwrap_or_default();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            p => stack.push(p),
        }
    }
    stack.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infra(name: &str, src: &str) -> FileExtract {
        let lang = gry_core::language::detect(name).unwrap();
        crate::extract::extract_file(name, lang, src.as_bytes())
    }

    #[test]
    fn dockerfile_images_env_and_entrypoint() {
        let src = "FROM golang:1.22 AS build\nFROM alpine:3.20\nENV PORT=8080\nENTRYPOINT [\"/app/server\"]\n";
        let out = infra("Dockerfile", src);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.label == "Image" && n.qualified_name.contains("golang")));
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.label == "Image" && n.qualified_name.contains("alpine")));
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.label == "EnvVar" && n.name == "PORT"));
        assert!(out.infra_nodes.iter().any(|n| n.label == "Process"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "DEPENDS_ON"));
    }

    #[test]
    fn compose_services_and_env() {
        let src = "services:\n  api:\n    image: acme/api:1\n    depends_on: [db]\n    environment:\n      DB_HOST: db\n  db:\n    image: postgres:16\n";
        let out = infra("docker-compose.yml", src);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.name == "api" && n.label == "Resource"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "DEPENDS_ON"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "CONNECTS_TO"));
    }

    #[test]
    fn kubernetes_service_maps_workload() {
        let src = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\n  labels:\n    app: api\nspec:\n  template:\n    metadata:\n      labels:\n        app: api\n    spec:\n      containers:\n      - name: api\n        image: acme/api:1\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: api\nspec:\n  selector:\n    app: api\n";
        let out = infra("deploy/api.yaml", src);
        assert!(out.infra_nodes.iter().any(|n| n.label == "Resource"
            && n.properties.get("kind").and_then(Value::as_str) == Some("Deployment")));
        assert!(out.infra_nodes.iter().any(|n| n.label == "Resource"
            && n.properties.get("kind").and_then(Value::as_str) == Some("Service")));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "INFRA_MAPS"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "USAGE"));
    }

    #[test]
    fn kustomize_imports_and_charts() {
        let out = infra(
            "deploy/kustomization.yaml",
            "resources:\n- app.yaml\nhelmCharts:\n- name: redis\n",
        );
        assert!(out
            .infra_edges
            .iter()
            .any(|e| e.edge_type == "IMPORTS" && e.target_qn.contains("app.yaml")));
        assert!(out
            .infra_edges
            .iter()
            .any(|e| e.edge_type == "DEPLOYS" && e.target_qn.contains("redis")));
    }

    #[test]
    fn make_targets_ignore_assignments() {
        let out = infra("Makefile", "CC := clang\nbuild:\n\tcc -o app main.c\n");
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.name == "build" && n.label == "Function"));
        assert!(!out.infra_nodes.iter().any(|n| n.name == "CC"));
    }

    #[test]
    fn make_globs_are_not_file_references() {
        let out = infra("Makefile", "test:\n\tgo test ./...\n");
        assert!(
            !out.infra_edges
                .iter()
                .any(|edge| edge.target_qn.contains("@file:...")),
            "go test ./... produced a file reference: {:?}",
            out.infra_edges
        );
        assert!(!out.infra_nodes.iter().any(|node| node.name == "..."));
        assert!(!out.infra_nodes.iter().any(|node| node.name == "./..."));
    }

    #[test]
    fn helm_template_images_and_values() {
        let template = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  template:\n    spec:\n      containers:\n      - name: api\n        image: {{ .Values.image.repository }}:{{ .Values.image.tag }}\n";
        let out = infra("chart/templates/deployment.yaml", template);
        assert!(out.infra_nodes.iter().any(|n| n.label == "Image"
            && n.properties.get("template").and_then(|v| v.as_str()) == Some("true")));
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.qualified_name == "__values__chart.image.repository"));
        assert!(out
            .infra_edges
            .iter()
            .any(|e| e.edge_type == "CONFIGURES" && e.target_qn.contains("image.repository")));

        let values = "image:\n  repository: acme/api\n  tag: \"1.2\"\n";
        let out = infra("chart/values.yaml", values);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.qualified_name == "__values__chart.image.repository"
                && n.properties.get("default").and_then(|v| v.as_str()) == Some("\"acme/api\"")));
    }

    #[test]
    fn kustomize_image_override() {
        let src = "namePrefix: prod-\nimages:\n  - name: acme/api\n    newName: registry/api\n    newTag: \"2.0\"\n";
        let out = infra("deploy/kustomization.yaml", src);
        assert!(out
            .infra_edges
            .iter()
            .any(|e| e.edge_type == "OVERRIDE" && e.target_qn.contains("registry")));
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.label == "Variable" && n.name == "prod-"));
    }

    #[test]
    fn terraform_variable_default_is_recorded() {
        let src = "variable \"region\" {\n  type = string\n  default = \"us-east-1\"\n}\nresource \"aws_s3_bucket\" \"b\" {\n  region = var.region\n}\n";
        let out = infra("main.tf", src);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.qualified_name == "terraform.variable.region"
                && n.properties.get("default").and_then(|v| v.as_str()) == Some("us-east-1")));
        assert!(out
            .infra_edges
            .iter()
            .any(|e| e.edge_type == "USAGE" && e.target_qn == "terraform.variable.region"));
    }

    #[test]
    fn dbt_refs_become_tables() {
        let src = "select * from {{ ref('orders') }} join {{ source('raw', 'users') }} u on true\n";
        let out = infra("models/orders.sql", src);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.name == "orders" && n.label == "Table"));
        assert!(out
            .infra_edges
            .iter()
            .any(|e| e.edge_type == "DEPENDS_ON" && e.target_qn.contains("raw.users")));
    }

    #[test]
    fn terraform_variables_and_modules() {
        let src = "variable \"region\" {}\nmodule \"net\" {\n  source = \"./modules/net\"\n  region = var.region\n}\n";
        let out = infra("infra/main.tf", src);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.label == "Variable" && n.name == "region"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "IMPORTS"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "USAGE"));
    }

    #[test]
    fn ci_jobs_deploy_manifests() {
        let src = "on: push\njobs:\n  deploy:\n    steps:\n    - run: kubectl apply -f deploy/app.yaml\n    - run: ./hack/deploy.sh\n";
        let out = infra(".github/workflows/deploy.yml", src);
        assert!(out
            .infra_nodes
            .iter()
            .any(|n| n.name == "deploy" && n.label == "Function"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "DEPLOYS"));
        assert!(out.infra_edges.iter().any(|e| e.edge_type == "SPAWNS"));
    }
}
