//! Source extraction: definitions, calls, imports, semantics and metrics.

use crate::model::*;
use crate::registry;
use gry_core::language::LanguageDef;
use gry_core::minhash::Fingerprint;
use gry_core::profile::AstProfile;
use gry_core::tokenize::split_text;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Map;
use serde_json::Value;
use std::collections::HashMap;

static TYPE_ASSIGN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:=|:=)\s*(?:&(?:mut\s+)?)?(?:[a-z_][a-z0-9_]*::)*([A-Z][A-Za-z0-9_]*)").unwrap()
});
static FILE_PATH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"["'](/[^"']*)["']"#).unwrap());
static RUST_MACRO_CALL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*(?:::|\.)\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap()
});
static DOC_BACKTICK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"`([A-Za-z_][A-Za-z0-9_:.]*)`").unwrap());
static DOC_BRACKET_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[([A-Za-z_][A-Za-z0-9_:.]*)\]").unwrap());
use tree_sitter::{Node, Parser, Tree};

pub fn extract_file(rel_path: &str, lang: &LanguageDef, source: &[u8]) -> FileExtract {
    extract_file_with_previous(rel_path, lang, source, None)
}

pub fn extract_file_with_previous(
    rel_path: &str,
    lang: &LanguageDef,
    source: &[u8],
    previous: Option<&FileExtract>,
) -> FileExtract {
    let mut out = FileExtract {
        language: lang.name.to_string(),
        sha256: sha256(source),
        size: source.len() as u64,
        lines: count_lines(source),
        ..Default::default()
    };
    match lang.name {
        "markdown" => crate::docs::parse_markdown(rel_path, source, &mut out),
        "rst" => crate::docs::parse_rst(rel_path, source, &mut out),
        "asciidoc" => crate::docs::parse_asciidoc(rel_path, source, &mut out),
        "pdf" => crate::docs::parse_pdf(rel_path, source, &mut out),
        "yaml" | "json" | "openapi" | "toml" | "ini" | "properties" | "dotenv" | "requirements"
        | "gitignore" | "gitattributes" | "gomod" => {
            crate::config::parse_config(rel_path, lang.name, source, &mut out);
            crate::docs::parse_openapi(rel_path, lang.name, source, &mut out);
            if lang.name == "yaml" {
                crate::infra::parse_infra(rel_path, lang.name, source, &mut out);
            }
        }
        "dockerfile" | "make" | "cmake" | "kustomize" | "kubernetes" | "helm" | "compose"
        | "github_actions" | "ansible" | "hcl" | "sql" => {
            crate::infra::parse_infra(rel_path, lang.name, source, &mut out);
            crate::config::parse_config(rel_path, lang.name, source, &mut out);
        }
        _ => {}
    }
    if lang.grammar.is_none() && lang.is_code() {
        crate::lexical::extract(rel_path, lang.name, source, &mut out);
    }
    if let Some(grammar) = lang.grammar {
        if let Some(language) = registry::grammar(grammar) {
            let mut parser = Parser::new();
            if parser.set_language(&language).is_ok() {
                let t_parse = std::time::Instant::now();
                if let Some(tree) = parser.parse(source, None) {
                    if std::env::var_os("GRY_PARSE_TIMING").is_some() {
                        eprintln!("[parse] {} tree-sitter {:?}", rel_path, t_parse.elapsed());
                    }
                    if lang.kind != gry_core::LanguageKind::Code
                        || matches!(
                            lang.name,
                            "yaml"
                                | "json"
                                | "toml"
                                | "ini"
                                | "dockerfile"
                                | "make"
                                | "hcl"
                                | "compose"
                                | "kubernetes"
                                | "kustomize"
                                | "helm"
                                | "github_actions"
                                | "ansible"
                        )
                    {
                        // Ir-format parsers are handled by their dedicated modules.
                    } else {
                        let t_walk = std::time::Instant::now();
                        let mut extractor = Extractor::new(rel_path, lang, source, &tree, previous);
                        extractor.run();
                        extractor.finish(&mut out);
                        if std::env::var_os("GRY_PARSE_TIMING").is_some() {
                            eprintln!("[parse] {} walk {:?}", rel_path, t_walk.elapsed());
                        }
                    }
                } else if lang.is_code() {
                    out.parse_gaps.push(ParseGap {
                        start_line: 1,
                        end_line: out.lines as i32,
                        reason: "parse_failed".to_string(),
                    });
                }
            }
        }
    }
    // File-level MinHash is derived on demand; per-symbol fingerprints are
    // stored during extraction.
    out
}

pub fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn count_lines(source: &[u8]) -> u32 {
    if source.is_empty() {
        0
    } else {
        let newlines = source.iter().filter(|b| **b == b'\n').count();
        (newlines + usize::from(!source.ends_with(b"\n"))) as u32
    }
}

struct Extractor<'a> {
    rel_path: &'a str,
    lang: &'a LanguageDef,
    source: &'a [u8],
    tree: &'a Tree,
    defs: Vec<Def>,
    calls: Vec<CallSite>,
    imports: Vec<Import>,
    usages: Vec<Usage>,
    routes: Vec<RouteDef>,
    env_accesses: Vec<EnvAccess>,
    channels: Vec<ChannelUse>,
    spawns: Vec<SpawnCall>,
    http_calls: Vec<HttpCall>,
    file_refs: Vec<FileRef>,
    type_map: HashMap<String, String>,
    return_types: HashMap<String, String>,
    file_docstring: Option<String>,
    gaps: Vec<ParseGap>,
    seen_defs: std::collections::HashSet<(String, i32)>,
    seen_usages: std::collections::HashSet<(String, String)>,
    previous: HashMap<(String, String), &'a Def>,
}

impl<'a> Extractor<'a> {
    fn new(
        rel_path: &'a str,
        lang: &'a LanguageDef,
        source: &'a [u8],
        tree: &'a Tree,
        previous: Option<&'a FileExtract>,
    ) -> Self {
        let mut previous_map = HashMap::new();
        if let Some(previous) = previous {
            for def in &previous.defs {
                previous_map
                    .entry((def.name.clone(), def.scope.join(".")))
                    .or_insert(def);
            }
        }
        Self {
            rel_path,
            lang,
            source,
            tree,
            defs: Vec::new(),
            calls: Vec::new(),
            imports: Vec::new(),
            usages: Vec::new(),
            routes: Vec::new(),
            env_accesses: Vec::new(),
            channels: Vec::new(),
            spawns: Vec::new(),
            http_calls: Vec::new(),
            file_refs: Vec::new(),
            type_map: HashMap::new(),
            return_types: HashMap::new(),
            file_docstring: leading_comments(source, 1, comment_markers(lang.name)),
            gaps: Vec::new(),
            seen_defs: std::collections::HashSet::new(),
            seen_usages: std::collections::HashSet::new(),
            previous: previous_map,
        }
    }

    fn run(&mut self) {
        let t = std::time::Instant::now();
        collect_type_map(self.tree.root_node(), self.source, &mut self.type_map);
        if matches!(
            self.lang.name,
            "python"
                | "javascript"
                | "typescript"
                | "tsx"
                | "java"
                | "csharp"
                | "go"
                | "rust"
                | "kotlin"
                | "swift"
                | "ruby"
                | "php"
                | "scala"
        ) {
            collect_return_types(
                self.tree.root_node(),
                self.source,
                self.lang.name,
                &mut self.return_types,
            );
        }
        if std::env::var_os("GRY_PARSE_TIMING").is_some() {
            eprintln!("[parse] type_map {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        let root = self.tree.root_node();
        self.walk(root, &mut Vec::new(), None, false, &[]);
        if std::env::var_os("GRY_PARSE_TIMING").is_some() {
            eprintln!("[parse] walk-only {:?}", t.elapsed());
        }
        let t = std::time::Instant::now();
        collect_parse_gaps(root, self.source, &mut self.gaps);
        if std::env::var_os("GRY_PARSE_TIMING").is_some() {
            eprintln!("[parse] gaps {:?}", t.elapsed());
        }
    }

    fn walk(
        &mut self,
        node: Node<'_>,
        scope: &mut Vec<String>,
        container: Option<&'static str>,
        in_function: bool,
        decorators: &[String],
    ) {
        // Language-specific definition forms first (Elixir def/defp, R assignment).
        if self.try_language_specific_def(node, scope, container, in_function) {
            return;
        }

        if node.kind() == "impl_item" && self.lang.name == "rust" {
            if let Some(type_node) = node.child_by_field_name("type") {
                let type_name = simple_type_name(text(self.source, type_node));
                if !type_name.is_empty() {
                    scope.push(type_name);
                    let mut child = node.walk();
                    for c in node.named_children(&mut child) {
                        self.walk(c, scope, Some("Struct"), in_function, &[]);
                    }
                    scope.pop();
                    return;
                }
            }
        }

        if let Some((label, is_function)) = classify_definition(node, self.lang.name, container) {
            let mut all_decorators = decorators.to_vec();
            all_decorators.extend(preceding_decorators(node, self.source));
            all_decorators.sort();
            all_decorators.dedup();
            if matches!(label, "Variable" | "Field") && in_function && container != Some("Class") {
                let mut child = node.walk();
                for c in node.named_children(&mut child) {
                    self.walk(c, scope, container, in_function, &[]);
                }
                return;
            }
            if let Some(def) =
                self.build_def(node, scope, container, label, is_function, &all_decorators)
            {
                let name = def.name.clone();
                if !name.is_empty() {
                    let key = (def.label.clone(), def.start_line);
                    if self.seen_defs.insert(key) {
                        self.defs.push(def);
                    }
                    // Recurse into the declaration's body with the symbol scope.
                    scope.push(name);
                    let next_container = if is_function {
                        container
                    } else if matches!(
                        label,
                        "Class" | "Struct" | "Interface" | "Trait" | "Enum" | "Type" | "Module"
                    ) {
                        Some(label)
                    } else {
                        container
                    };
                    let mut child = node.walk();
                    for c in node.named_children(&mut child) {
                        self.walk(c, scope, next_container, in_function || is_function, &[]);
                    }
                    scope.pop();
                    return;
                }
            }
        }

        // Non-definition nodes: collect signals, then descend.
        self.collect_node_signals(node, scope, in_function);
        let next_decorators = decorator_names(node, self.source);
        let mut child = node.walk();
        for c in node.named_children(&mut child) {
            let child_decorators = if next_decorators.is_empty() {
                &[][..]
            } else {
                &next_decorators[..]
            };
            self.walk(c, scope, container, in_function, child_decorators);
        }
    }

    fn try_language_specific_def(
        &mut self,
        node: Node<'_>,
        scope: &mut Vec<String>,
        container: Option<&'static str>,
        in_function: bool,
    ) -> bool {
        match self.lang.name {
            "elixir" if node.kind() == "call" => {
                let head = text(self.source, node)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let label = match head.as_str() {
                    "defmodule" => "Module",
                    "def" | "defp" | "defmacro" | "defmacrop" => {
                        if in_function {
                            "Function"
                        } else {
                            "Function"
                        }
                    }
                    _ => return false,
                };
                let Some(args) = node.child_by_field_name("arguments") else {
                    return false;
                };
                let first = args.named_child(0).unwrap_or(args);
                let name = match label {
                    "Module" => text(self.source, first)
                        .trim()
                        .trim_matches('"')
                        .to_string(),
                    _ => {
                        let raw = text(self.source, first);
                        extract_callable_name(raw)
                    }
                };
                if name.is_empty() {
                    return false;
                }
                let mut def = Def {
                    label: label.to_string(),
                    name: name.clone(),
                    scope: scope.clone(),
                    start_line: line(node),
                    end_line: line_end(node),
                    signature: Some(first_line_window(self.source, node, 160)),
                    ..Default::default()
                };
                def.is_test = false;
                def.docstring = preceding_docstring(self.source, node, self.lang.name);
                self.defs.push(def);
                scope.push(name);
                let mut child = node.walk();
                for c in node.named_children(&mut child) {
                    self.walk(
                        c,
                        scope,
                        if label == "Module" {
                            Some("Module")
                        } else {
                            container
                        },
                        in_function,
                        &[],
                    );
                }
                scope.pop();
                true
            }
            "r" if node.kind() == "binary_operator" => {
                let op = node
                    .child_by_field_name("operator")
                    .map(|n| text(self.source, n))
                    .unwrap_or("");
                if op != "<-" && op != "=" {
                    return false;
                }
                let rhs = node.child_by_field_name("rhs");
                let Some(rhs) = rhs else { return false };
                if rhs.kind() != "function_definition" && rhs.kind() != "function" {
                    return false;
                }
                let lhs = node
                    .child_by_field_name("lhs")
                    .map(|n| text(self.source, n).trim().to_string())
                    .unwrap_or_default();
                if lhs.is_empty() {
                    return false;
                }
                let mut def = Def {
                    label: "Function".to_string(),
                    name: lhs.trim_matches('`').to_string(),
                    scope: scope.clone(),
                    start_line: line(node),
                    end_line: line_end(node),
                    signature: Some(first_line_window(self.source, node, 160)),
                    ..Default::default()
                };
                def.params = extract_params(rhs, self.source);
                def.docstring = preceding_docstring(self.source, node, "r");
                self.defs.push(def);
                return true;
            }
            _ => false,
        }
    }

    fn build_def(
        &mut self,
        node: Node<'_>,
        scope: &[String],
        container: Option<&'static str>,
        label: &'static str,
        is_function: bool,
        decorators: &[String],
    ) -> Option<Def> {
        let mut name = definition_name(node, self.source, self.lang.name)?;
        let mut def_scope = scope.to_vec();
        // Qualified C/C++/Ruby/etc. names carry their container in the name.
        if let Some((qualifier, short)) = split_qualified_name(&name) {
            if !qualifier.is_empty() {
                def_scope.extend(qualifier.split('.').map(|s| s.to_string()));
                name = short;
            }
        }
        // Go methods: receiver type belongs to the QN scope.
        if self.lang.name == "go" && label == "Method" {
            if let Some(recv) = node.child_by_field_name("receiver") {
                let r = simple_type_name(text(self.source, recv));
                if !r.is_empty() && !def_scope.contains(&r) {
                    def_scope.push(r);
                }
            }
        }
        let actual_label = if is_function {
            if label == "Method"
                || matches!(
                    container,
                    Some("Class") | Some("Struct") | Some("Interface") | Some("Trait")
                )
            {
                "Method"
            } else {
                "Function"
            }
        } else {
            label
        };
        let docstring = if self.lang.name == "python" {
            python_docstring(node, self.source)
                .or_else(|| preceding_docstring(self.source, node, self.lang.name))
        } else {
            preceding_docstring(self.source, node, self.lang.name)
        };
        let mut def = Def {
            label: actual_label.to_string(),
            name: name.clone(),
            scope: def_scope,
            start_line: line(node),
            end_line: line_end(node),
            docstring,
            signature: Some(first_line_window(self.source, node, 240)),
            params: if is_function {
                extract_params(node, self.source)
            } else {
                Vec::new()
            },
            return_type: if is_function {
                extract_return_type(node, self.source)
            } else {
                None
            },
            bases: extract_bases(node, self.source, self.lang.name),
            decorators: decorators.to_vec(),
            ..Default::default()
        };
        if is_function {
            let body = function_body(node);
            if let Some(body) = body {
                let body_text = text(self.source, body);
                let body_hash =
                    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(body_text.as_bytes()));
                def.properties
                    .insert("body_hash".to_string(), Value::String(body_hash.clone()));
                let key = (def.name.clone(), def.scope.join("."));
                let reusable = self.previous.get(&key).filter(|prev| {
                    prev.properties.get("body_hash").and_then(Value::as_str)
                        == Some(body_hash.as_str())
                        && prev.properties.get("fp").is_some()
                });
                if let Some(prev) = reusable {
                    def.complexity = prev.complexity;
                    def.cognitive = prev.cognitive;
                    def.loop_depth = prev.loop_depth;
                    for key in ["sp", "body_lines", "body_tokens", "fp"] {
                        if let Some(value) = prev.properties.get(key) {
                            def.properties.insert(key.to_string(), value.clone());
                        }
                    }
                } else {
                    let profile = compute_profile(body, self.source, &def.params);
                    def.complexity = Some(
                        profile.if_count
                            + profile.for_count
                            + profile.while_count
                            + profile.switch_count
                            + profile.try_count
                            + 1,
                    );
                    def.cognitive = Some(
                        profile.if_count
                            + profile.for_count
                            + profile.while_count
                            + profile.switch_count * 2
                            + profile.max_nesting_depth,
                    );
                    def.loop_depth = Some(profile.for_count.max(profile.while_count));
                    def.properties
                        .insert("sp".to_string(), Value::String(profile.to_string_compact()));
                    def.properties.insert(
                        "body_lines".to_string(),
                        Value::Number((line_end(node) - line(node) + 1).into()),
                    );
                    def.properties.insert(
                        "body_tokens".to_string(),
                        Value::Number(profile.body_tokens.into()),
                    );
                    let tokens = split_text(text(self.source, body));
                    def.properties.insert(
                        "fp".to_string(),
                        Value::String(Fingerprint::from_tokens(&tokens).to_hex()),
                    );
                }
            }
        }
        if let Some(doc) = &def.docstring {
            def.properties
                .insert("docstring".to_string(), Value::String(doc.clone()));
            for raw in extract_doc_refs(doc) {
                self.usages.push(Usage {
                    name: raw,
                    scope: def.scope.clone(),
                    line: def.start_line,
                    kind: "doc_ref".to_string(),
                });
            }
        }
        def.is_test = is_test_def(&def, self.rel_path, self.lang.name);
        def.is_entry_point = is_entry_point_name(&def.name, self.lang.name);
        def.exported = is_exported(node, self.source, self.lang.name);
        if !def.is_test && is_test_file(self.rel_path) {
            def.is_test = true;
        }
        if def.is_test {
            def.properties
                .insert("is_test".to_string(), Value::Bool(true));
        }
        if def.is_entry_point {
            def.properties
                .insert("is_entry_point".to_string(), Value::Bool(true));
        }
        if let Some(vis) = visibility(node, self.source) {
            def.properties
                .insert("visibility".to_string(), Value::String(vis));
        }
        Some(def)
    }

    fn collect_node_signals(&mut self, node: Node<'_>, scope: &[String], in_function: bool) {
        let kind = node.kind();
        if is_import_node(kind, self.lang.name) {
            if let Some(import) = parse_import(node, self.source, self.lang.name) {
                self.imports.push(import);
            }
        }
        if is_call_node(kind) {
            if let Some(call) = parse_call(
                node,
                self.source,
                self.lang.name,
                scope,
                &self.type_map,
                &self.return_types,
            ) {
                // A call can also be a route/env/channel/spawn/http signal.
                self.record_call_signals(&call);
                self.calls.push(call);
            }
        }
        if is_decorator_node(kind) {
            if let Some(route) = route_from_decorator(node, self.source, scope) {
                self.routes.push(route);
            }
        }
        if kind == "string" || kind == "interpreted_string_literal" || kind == "string_literal" {
            let raw = unquote(text(self.source, node));
            if looks_like_file_path(&raw) {
                self.file_refs.push(FileRef {
                    path: raw,
                    line: line(node),
                });
            }
        }
        if self.lang.name == "rust" && kind == "macro_invocation" {
            self.collect_rust_macro_calls(node, scope);
        }
        if matches!(
            kind,
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "property_identifier"
                | "constant"
                | "simple_identifier"
        ) {
            self.collect_usage(node, scope);
        }
        if kind == "member_expression" && self.lang.name == "javascript" {
            if let Some(name) = self.record_js_env_member(node, scope) {
                self.env_accesses.push(EnvAccess {
                    key: name,
                    line: line(node),
                    scope: scope.to_vec(),
                    mode: "read".to_string(),
                });
            }
        }
        let _ = in_function;
    }

    fn record_call_signals(&mut self, call: &CallSite) {
        let name = call.name.as_str();
        let lower = name.to_ascii_lowercase();
        // Environment reads.
        if matches!(
            lower.as_str(),
            "getenv" | "getenvs" | "getenvironmentvariable" | "var" | "env" | "environ_get"
        ) || lower == "get"
        {
            if let Some(arg) = call.args.first() {
                if let Some(key) = literal_string(arg) {
                    self.env_accesses.push(EnvAccess {
                        key,
                        line: call.line,
                        scope: call.scope.clone(),
                        mode: "read".to_string(),
                    });
                }
            }
        }
        if call.receiver.as_deref() == Some("process.env") || name == "env" {
            if let Some(arg) = call.args.first() {
                if let Some(key) = literal_string(arg) {
                    self.env_accesses.push(EnvAccess {
                        key,
                        line: call.line,
                        scope: call.scope.clone(),
                        mode: "read".to_string(),
                    });
                }
            }
        }
        // Channel participation.
        let action = match lower.as_str() {
            "emit" | "publish" | "send" | "produce" | "dispatch" => Some("emit"),
            "on" | "subscribe" | "listen" | "consume" | "addlistener" => Some("listen"),
            _ => None,
        };
        if let Some(action) = action {
            if let Some(arg) = call.args.first() {
                if let Some(channel) = literal_string(arg) {
                    let broker = if lower == "publish" || lower == "subscribe" {
                        "pubsub"
                    } else {
                        "events"
                    };
                    self.channels.push(ChannelUse {
                        name: channel,
                        line: call.line,
                        scope: call.scope.clone(),
                        action: action.to_string(),
                        broker: broker.to_string(),
                    });
                }
            }
        }
        // Process spawn.
        if matches!(
            lower.as_str(),
            "run"
                | "popen"
                | "call"
                | "check_output"
                | "check_call"
                | "spawn"
                | "exec"
                | "system"
                | "execsync"
                | "spawnsync"
                | "command"
                | "new"
                | "start"
                | "processstart"
        ) && is_spawn_callee(call)
        {
            let program = call
                .args
                .first()
                .and_then(|a| literal_string(a))
                .unwrap_or_else(|| "<dynamic>".to_string());
            self.spawns.push(SpawnCall {
                dynamic: program == "<dynamic>",
                program,
                line: call.line,
                scope: call.scope.clone(),
            });
        }
        // Route registration calls are not outbound HTTP calls.
        let route_registration = route_from_call(call, self.lang.name);
        if let Some(route) = route_registration {
            self.routes.push(route);
        } else if let Some(url) = call
            .args
            .first()
            .and_then(|a| literal_string(a).or_else(|| url_prefix(a)))
        {
            if url.starts_with("http://") || url.starts_with("https://") || url.starts_with('/') {
                let method = if matches!(
                    lower.as_str(),
                    "post" | "put" | "patch" | "delete" | "head" | "options"
                ) {
                    lower.to_ascii_uppercase()
                } else if call
                    .receiver
                    .as_deref()
                    .is_some_and(|r| r.contains("requests") || r.contains("axios"))
                {
                    if name == "request" {
                        call.args
                            .get(1)
                            .and_then(|a| literal_string(a))
                            .unwrap_or_else(|| "GET".to_string())
                            .to_ascii_uppercase()
                    } else {
                        name.to_ascii_uppercase()
                    }
                } else {
                    "GET".to_string()
                };
                if is_http_client(call) {
                    let (host, path) = split_http_url(&url);
                    self.http_calls.push(HttpCall {
                        method,
                        url_path: path,
                        host,
                        line: call.line,
                        scope: call.scope.clone(),
                    });
                }
            }
        }
    }

    fn collect_usage(&mut self, node: Node<'_>, scope: &[String]) {
        if let Some(parent) = node.parent() {
            let parent_kind = parent.kind();
            if parent_kind.contains("definition")
                || parent_kind.contains("declaration")
                || parent_kind.ends_with("_item")
                || parent_kind.contains("parameter")
            {
                if parent
                    .child_by_field_name("name")
                    .map(|n| n.id() == node.id())
                    .unwrap_or(false)
                {
                    return;
                }
                if parent_kind.contains("parameter") {
                    return;
                }
            }
            if parent_kind == "call_expression"
                && parent
                    .child_by_field_name("function")
                    .map(|n| n.id() == node.id())
                    .unwrap_or(false)
            {
                return;
            }
            if matches!(
                parent_kind,
                "import_statement" | "use_declaration" | "preproc_include"
            ) {
                return;
            }
        }
        let name = text(self.source, node).trim();
        if name.is_empty() || name.len() > 200 {
            return;
        }
        let scope_key = scope.join(".");
        if !self.seen_usages.insert((scope_key, name.to_string())) {
            return;
        }
        self.usages.push(Usage {
            name: name.to_string(),
            scope: scope.to_vec(),
            line: line(node),
            kind: "usage".to_string(),
        });
    }

    fn collect_rust_macro_calls(&mut self, node: Node<'_>, scope: &[String]) {
        let raw = text(self.source, node);
        for cap in RUST_MACRO_CALL_RE.captures_iter(raw) {
            let receiver = cap.get(1).unwrap().as_str().to_string();
            let name = cap.get(2).unwrap().as_str().to_string();
            if matches!(
                name.as_str(),
                "assert"
                    | "assert_eq"
                    | "assert_ne"
                    | "format"
                    | "print"
                    | "println"
                    | "write"
                    | "writeln"
                    | "vec"
                    | "panic"
            ) {
                continue;
            }
            let offset_line = raw[..cap.get(0).unwrap().start()].matches('\n').count() as i32;
            let receiver_type = self.type_map.get(&receiver).cloned().or_else(|| {
                if receiver.chars().next().is_some_and(|c| c.is_uppercase()) {
                    Some(receiver.clone())
                } else {
                    None
                }
            });
            self.calls.push(CallSite {
                name,
                receiver: Some(receiver),
                receiver_type,
                args: Vec::new(),
                scope: scope.to_vec(),
                line: line(node) + offset_line,
                kind: "method".to_string(),
                is_reference: false,
                properties: Map::new(),
            });
        }
    }

    fn record_js_env_member(&mut self, node: Node<'_>, scope: &[String]) -> Option<String> {
        let text = text(self.source, node);
        if let Some(rest) = text.strip_prefix("process.env.") {
            let key = rest.split(['.', '(', ' ', ';']).next().unwrap_or("");
            if !key.is_empty() {
                let _ = scope;
                return Some(key.to_string());
            }
        }
        None
    }

    fn finish(self, out: &mut FileExtract) {
        out.defs = self.defs;
        out.calls = self.calls;
        out.imports = self.imports;
        out.usages = self.usages;
        out.routes = self.routes;
        out.env_accesses = self.env_accesses;
        out.channels = self.channels;
        out.spawns = self.spawns;
        out.http_calls = self.http_calls;
        out.file_refs = self.file_refs;
        out.file_docstring = self.file_docstring;
        out.parse_gaps = self.gaps;
    }
}

fn classify_definition(
    node: Node<'_>,
    lang: &str,
    container: Option<&'static str>,
) -> Option<(&'static str, bool)> {
    let kind = node.kind();
    // Never treat unresolved error nodes as definitions.
    if kind == "ERROR" || kind == "MISSING" {
        return None;
    }
    let function = matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "function_item"
            | "function_signature_item"
            | "method_declaration"
            | "method_definition"
            | "method"
            | "singleton_method"
            | "constructor_declaration"
            | "constructor_definition"
            | "function_statement"
            | "function_declaration_statement"
            | "procedure_declaration"
            | "subroutine_declaration"
            | "fn_decl"
            | "local_function_statement"
            | "init_declaration"
            | "deinit_declaration"
            | "function_definition_statement"
    ) || (kind == "function" && matches!(lang, "r" | "elixir"));
    if function {
        let label = if container == Some("Class")
            || container == Some("Struct")
            || container == Some("Interface")
            || container == Some("Trait")
        {
            "Method"
        } else {
            "Function"
        };
        return Some((label, true));
    }
    let field_like = matches!(
        kind,
        "field_declaration"
            | "field_definition"
            | "public_field_definition"
            | "property_declaration"
            | "property_definition"
            | "property_element"
    );
    if field_like {
        return Some(("Field", false));
    }
    let constant_like = matches!(
        kind,
        "const_item" | "constant_declaration" | "const_declaration" | "const_spec" | "enum_member"
    );
    if constant_like {
        return Some(("Constant", false));
    }
    let variable_like = matches!(
        kind,
        "variable_declarator"
            | "static_item"
            | "var_spec"
            | "global_variable_declaration"
            | "module_variable"
    );
    if variable_like {
        return Some(("Variable", false));
    }
    let macro_like = matches!(
        kind,
        "macro_definition" | "preproc_def" | "preproc_function_def" | "macro_rule"
    );
    if macro_like {
        return Some(("Macro", false));
    }
    if matches!(kind, "assignment" | "augmented_assignment") && container == Some("Class") {
        return Some(("Field", false));
    }
    let class_like = match kind {
        "class_declaration"
        | "class_definition"
        | "class_specifier"
        | "class_statement"
        | "class"
        | "abstract_class_declaration" => Some("Class"),
        "struct_declaration" | "struct_specifier" | "struct_item" | "struct_definition" => {
            Some("Struct")
        }
        "interface_declaration" | "protocol_declaration" | "interface_definition" => {
            Some("Interface")
        }
        "trait_declaration" | "trait_item" | "trait_definition" => Some("Trait"),
        "enum_declaration" | "enum_item" | "enum_specifier" | "enum_definition" => Some("Enum"),
        "type_alias_declaration" | "type_item" | "type_declaration" | "type_definition" => {
            Some("Type")
        }
        "record_declaration" | "record_struct_declaration" => Some("Struct"),
        "annotation_type_declaration" => Some("Interface"),
        "object_declaration" | "object_definition" => Some("Class"),
        "union_specifier" => Some("Struct"),
        "contract_declaration" => Some("Class"),
        "module_declaration" | "mod_item" | "module_expression" => Some("Module"),
        _ => None,
    };
    class_like.map(|label| (label, false))
}

fn definition_name(node: Node<'_>, source: &[u8], lang: &str) -> Option<String> {
    for field in ["name", "declarator", "pattern", "left", "type"] {
        if let Some(child) = node.child_by_field_name(field) {
            if let Some(name) = name_from_node(child, source) {
                return Some(name);
            }
        }
    }
    if matches!(lang, "lua" | "elixir") {
        if let Some(name) = name_from_node(node, source) {
            return Some(name);
        }
    }
    None
}

fn name_from_node(node: Node<'_>, source: &[u8]) -> Option<String> {
    let kind = node.kind();
    if matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "namespace_identifier"
            | "constant"
            | "name"
            | "variable_name"
            | "label_name"
            | "simple_identifier"
            | "word"
    ) {
        let t = text(source, node).trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if matches!(
        kind,
        "qualified_identifier"
            | "scoped_identifier"
            | "qualified_name"
            | "nested_identifier"
            | "member_expression"
    ) {
        if let Some(last) = node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("property"))
            .and_then(|n| name_from_node(n, source))
        {
            return Some(last);
        }
        let t = text(source, node).trim();
        return Some(extract_callable_name(t));
    }
    for field in ["declarator", "name", "pattern", "type", "value"] {
        if let Some(child) = node.child_by_field_name(field) {
            if child.id() != node.id() {
                if let Some(name) = name_from_node(child, source) {
                    return Some(name);
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "property_identifier"
                | "simple_identifier"
                | "constant"
        ) {
            return Some(text(source, child).trim().to_string());
        }
    }
    None
}

fn text<'a>(source: &'a [u8], node: Node<'_>) -> &'a str {
    let start = node.start_byte().min(source.len());
    let end = node.end_byte().min(source.len());
    std::str::from_utf8(&source[start..end]).unwrap_or("")
}

fn line(node: Node<'_>) -> i32 {
    node.start_position().row as i32 + 1
}

fn line_end(node: Node<'_>) -> i32 {
    node.end_position().row as i32 + 1
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(max).collect()
}

fn simple_type_name(raw: &str) -> String {
    let raw = raw.trim().trim_matches(['&', '*', ' ', '!']);
    let last = raw
        .split(['.', ':', '<', ' ', ',', '>', '[', ']'])
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(raw);
    last.to_string()
}

fn split_qualified_name(name: &str) -> Option<(String, String)> {
    let idx = name.rfind("::").or_else(|| name.rfind('.'))?;
    let qualifier = name[..idx].trim_matches(':').replace("::", ".");
    let short = name[idx..].trim_start_matches([':', '.']).to_string();
    if qualifier.is_empty() || short.is_empty() {
        None
    } else {
        Some((qualifier, short))
    }
}

fn extract_callable_name(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_prefix("new ").unwrap_or(raw);
    let raw = raw.trim_end_matches(['!', '?', '(', ')']);
    let mut last = raw;
    for sep in ["->", "::", "."] {
        if let Some(idx) = raw.rfind(sep) {
            last = &raw[idx + sep.len()..];
        }
    }
    let last = last.trim();
    // Strip generic arguments and macro delimiters.
    let last = last.split('<').next().unwrap_or(last);
    let last = last.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '$');
    if last.is_empty() {
        raw.split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or("")
            .to_string()
    } else {
        last.to_string()
    }
}

fn is_call_node(kind: &str) -> bool {
    matches!(
        kind,
        "call_expression"
            | "call"
            | "method_invocation"
            | "function_call"
            | "invocation_expression"
            | "new_expression"
            | "object_creation_expression"
            | "macro_invocation"
            | "constructor_invocation"
            | "explicit_constructor_invocation"
            | "command"
            | "command_substitution"
            | "call_expression_statement"
    )
}

fn parse_call(
    node: Node<'_>,
    source: &[u8],
    lang: &str,
    scope: &[String],
    type_map: &HashMap<String, String>,
    return_types: &HashMap<String, String>,
) -> Option<CallSite> {
    let is_new = matches!(node.kind(), "new_expression" | "object_creation_expression");
    let function = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("method"))
        .or_else(|| node.child_by_field_name("name"))
        .or_else(|| node.child_by_field_name("callee"))
        .or_else(|| node.child_by_field_name("type"))
        .or_else(|| node.child_by_field_name("target"))
        .or_else(|| node.named_child(0))?;
    let raw = text(source, function).trim();
    if raw.is_empty() {
        return None;
    }
    let name = extract_callable_name(raw);
    if name.is_empty() {
        return None;
    }
    let receiver = receiver_text(raw, &name);
    let receiver_type = receiver.as_ref().and_then(|r| {
        let r = r.trim();
        if r == "self" || r == "this" {
            scope
                .iter()
                .rev()
                .nth(1)
                .cloned()
                .or_else(|| scope.last().cloned())
        } else {
            type_map
                .get(r)
                .cloned()
                .or_else(|| type_map.get(r.trim_start_matches("&")).cloned())
                .or_else(|| {
                    let base = r.split('(').next().unwrap_or(r).trim();
                    let base = base.rsplit(['.', ':']).next().unwrap_or(base);
                    return_types.get(base).cloned()
                })
        }
    });
    let args = node
        .child_by_field_name("arguments")
        .or_else(|| node.child_by_field_name("args"))
        .or_else(|| {
            find_child_kind(
                node,
                &["argument_list", "arguments", "argument_list_expression"],
            )
        })
        .map(|a| split_args(text(source, a)))
        .unwrap_or_default();
    let kind = if is_new {
        "constructor"
    } else if raw.contains('!') && lang == "rust" {
        "macro"
    } else if receiver.is_some() {
        "method"
    } else {
        "call"
    };
    Some(CallSite {
        name,
        receiver,
        receiver_type,
        args,
        scope: scope.to_vec(),
        line: line(node),
        kind: kind.to_string(),
        is_reference: false,
        properties: Map::new(),
    })
}

fn receiver_text(raw: &str, name: &str) -> Option<String> {
    let idx = raw.rfind(name)?;
    let before = raw[..idx].trim_end_matches(['.', ':', '-', '>', '?', '!', ' ']);
    let before = before.trim();
    if before.is_empty() || matches!(before, "return" | "await" | "new") {
        None
    } else if before.contains(' ') && !before.contains('.') {
        None
    } else {
        Some(before.to_string())
    }
}

fn split_args(raw: &str) -> Vec<String> {
    let inner = raw
        .trim()
        .trim_start_matches(['(', '[', ' '])
        .trim_end_matches([')', ']', ' ']);
    if inner.trim().is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut current = String::new();
    for ch in inner.chars() {
        if let Some(quote) = in_string {
            current.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => {
                in_string = Some(ch);
                current.push(ch);
            }
            '(' | '[' | '{' | '<' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' | '}' | '>' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn extract_params(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let params = node
        .child_by_field_name("parameters")
        .or_else(|| node.child_by_field_name("parameter"))
        .or_else(|| node.child_by_field_name("parameters_list"))
        .or_else(|| find_child_kind(node, &["parameters", "parameter_list", "formal_parameters"]));
    let Some(params) = params else {
        return Vec::new();
    };
    split_args(text(source, params))
        .into_iter()
        .filter_map(|p| {
            let cleaned = p
                .split(':')
                .next()
                .unwrap_or(&p)
                .split('=')
                .next()
                .unwrap_or(&p)
                .trim()
                .trim_start_matches(['*', '&'])
                .trim_start_matches("mut ")
                .to_string();
            let name = cleaned
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if name.is_empty() || name == "self" || name == "this" {
                None
            } else {
                Some(name)
            }
        })
        .collect()
}

fn extract_return_type(node: Node<'_>, source: &[u8]) -> Option<String> {
    for field in ["return_type", "result", "type"] {
        if let Some(child) = node.child_by_field_name(field) {
            let raw = text(source, child).trim();
            if !raw.is_empty() && raw != node.kind() {
                return Some(simple_type_name(raw));
            }
        }
    }
    None
}

fn extract_bases(node: Node<'_>, source: &[u8], lang: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for field in [
        "superclass",
        "superclasses",
        "bases",
        "interfaces",
        "extends",
        "implements",
        "traits",
        "protocols",
    ] {
        if let Some(child) = node.child_by_field_name(field) {
            let implements = field == "interfaces"
                || field == "implements"
                || field == "traits"
                || field == "protocols";
            for name in split_args(text(source, child)) {
                let name = name
                    .split_whitespace()
                    .last()
                    .unwrap_or(&name)
                    .trim()
                    .to_string();
                if !name.is_empty() && name != "implements" && name != "extends" {
                    out.push((simple_type_name(&name), implements));
                }
            }
        }
    }
    // Children clauses (extends_clause / implements_clause / inheritance_specifier).
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        if kind.contains("extends")
            || kind.contains("implements")
            || kind.contains("inheritance")
            || kind.contains("superclass")
            || kind.contains("base_class")
            || kind.contains("interface")
            || kind.contains("trait")
        {
            let implements =
                kind.contains("implements") || kind.contains("interface") || kind.contains("trait");
            for name in split_args(text(source, child)) {
                let cleaned = name
                    .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_')
                    .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
                if cleaned.is_empty() {
                    continue;
                }
                let simple = simple_type_name(cleaned);
                if !out.iter().any(|(n, _)| n == &simple) {
                    out.push((simple, implements));
                }
            }
        }
    }
    let _ = lang;
    out
}

fn function_body<'a>(node: Node<'a>) -> Option<Node<'a>> {
    for field in ["body", "block", "statement_block", "statements"] {
        if let Some(child) = node.child_by_field_name(field) {
            return Some(child);
        }
    }
    find_child_kind(
        node,
        &[
            "block",
            "statement_block",
            "body",
            "function_body",
            "compound_statement",
            "do_block",
        ],
    )
}

fn find_child_kind<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
    }
    None
}

fn compute_profile(body: Node<'_>, source: &[u8], params: &[String]) -> AstProfile {
    let mut profile = AstProfile {
        param_count: params.len() as u32,
        body_lines: (line_end(body) - line(body) + 1).max(0) as u32,
        ..Default::default()
    };
    let mut stack: Vec<(Node<'_>, u32)> = vec![(body, 0)];
    let mut depth_sum = 0u32;
    let mut depth_count = 0u32;
    let param_set: std::collections::HashSet<&str> = params.iter().map(String::as_str).collect();
    while let Some((node, depth)) = stack.pop() {
        profile.body_tokens += 1;
        let kind = node.kind();
        let is_branch = kind.contains("if")
            || kind.contains("for")
            || kind.contains("while")
            || kind.contains("switch")
            || kind.contains("catch")
            || kind.contains("match");
        match kind {
            k if k.contains("if") && !k.contains("elif") => profile.if_count += 1,
            k if k.contains("for") => profile.for_count += 1,
            k if k.contains("while") => profile.while_count += 1,
            k if k.contains("switch") => profile.switch_count += 1,
            k if k.contains("catch") || k.contains("except") => profile.try_count += 1,
            k if k == "return_statement" || k == "return" => profile.return_count += 1,
            k if k.contains("assignment") => profile.assignment_count += 1,
            k if k == "string" || k.contains("string_literal") => profile.string_literals += 1,
            k if k == "number" || k.contains("integer_literal") || k.contains("float_literal") => {
                profile.number_literals += 1
            }
            k if k == "true" || k == "false" || k.contains("boolean") => profile.bool_literals += 1,
            k if k == "binary_expression" || k == "binary_operator" => profile.arithmetic_ops += 1,
            _ => {}
        }
        if is_branch {
            let d = depth + 1;
            depth_sum += d;
            depth_count += 1;
            profile.max_nesting_depth = profile.max_nesting_depth.max(d);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if param_set.contains(text(source, child).trim()) {
                if node.kind().contains("return") {
                    profile.params_in_returns += 1;
                }
                if node.kind().contains("if") || node.kind().contains("condition") {
                    profile.params_in_conditions += 1;
                }
            }
            stack.push((child, if is_branch { depth + 1 } else { depth }));
        }
    }
    if depth_count > 0 {
        profile.avg_nesting_depth_x10 = (depth_sum * 10) / depth_count;
    }
    profile.unique_operators =
        profile.arithmetic_ops + profile.logical_ops + profile.comparison_ops;
    profile.unique_operands = profile.body_tokens.saturating_sub(profile.unique_operators);
    profile.total_operators = profile.unique_operators;
    profile.total_operands = profile.body_tokens;
    profile
}

fn is_import_node(kind: &str, lang: &str) -> bool {
    let generic = kind.contains("import")
        || kind.contains("include")
        || kind.contains("require")
        || kind == "use_declaration"
        || kind == "using_directive"
        || kind == "extern_crate_declaration";
    if !generic {
        return false;
    }
    // Do not treat a call named `require` as an import node; calls are handled
    // separately and converted in `record_call_signals`.
    if matches!(kind, "call" | "call_expression" | "function_call") {
        return false;
    }
    let _ = lang;
    true
}

fn parse_import(node: Node<'_>, source: &[u8], lang: &str) -> Option<Import> {
    let raw = text(source, node).trim();
    if raw.is_empty() {
        return None;
    }
    let line = line(node);
    let unquoted = |s: &str| unquote(s.trim());
    // Python.
    if lang == "python" {
        if node.kind() == "import_from_statement" {
            let module = node
                .child_by_field_name("module_name")
                .map(|n| text(source, n).to_string())
                .unwrap_or_else(|| {
                    raw.trim_start_matches("from ")
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string()
                });
            let names = node
                .child_by_field_name("name")
                .map(|n| text(source, n))
                .unwrap_or("");
            let local = names
                .split(',')
                .next()
                .map(|n| {
                    n.trim()
                        .split(" as ")
                        .last()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
                .filter(|s| !s.is_empty() && s != "*");
            return Some(Import {
                is_relative: module.starts_with('.'),
                specifier: module,
                local_name: local,
                imported_name: names.split(',').next().map(|s| s.trim().to_string()),
                line,
                kind: "from".to_string(),
                ..Default::default()
            });
        }
        let module = raw.trim_start_matches("import ").trim();
        let first = module.split(',').next().unwrap_or(module).trim();
        let local = first
            .split(" as ")
            .last()
            .unwrap_or(first)
            .trim()
            .to_string();
        return Some(Import {
            is_relative: first.starts_with('.'),
            specifier: first
                .split(" as ")
                .next()
                .unwrap_or(first)
                .trim()
                .to_string(),
            local_name: Some(local),
            imported_name: None,
            line,
            kind: "import".to_string(),
            ..Default::default()
        });
    }
    // C/C++ include.
    if lang == "c" || lang == "cpp" || lang == "cuda" {
        let spec = raw
            .trim_start_matches("#include")
            .trim()
            .trim_start_matches(['<', '"'])
            .trim_end_matches(['>', '"'])
            .to_string();
        let base = spec.rsplit('/').next().unwrap_or(&spec).to_string();
        return Some(Import {
            is_relative: raw.contains('"'),
            specifier: spec,
            local_name: Some(base),
            imported_name: None,
            line,
            kind: "include".to_string(),
            ..Default::default()
        });
    }
    // Go import spec.
    if lang == "go" && node.kind().contains("import_spec") {
        let spec = node
            .child_by_field_name("path")
            .map(|n| unquoted(text(source, n)))
            .unwrap_or_else(|| unquoted(raw));
        let alias = node
            .child_by_field_name("name")
            .map(|n| text(source, n).trim().to_string());
        let local = alias.or_else(|| {
            spec.rsplit('/')
                .next()
                .map(|s| s.trim_matches('"').to_string())
        });
        return Some(Import {
            is_relative: false,
            specifier: spec,
            local_name: local,
            imported_name: None,
            line,
            kind: "import".to_string(),
            ..Default::default()
        });
    }
    // Rust use / extern crate.
    if lang == "rust" {
        let body = raw
            .trim_start_matches("pub ")
            .trim_start_matches("use ")
            .trim_start_matches("extern crate ")
            .trim_end_matches(';')
            .trim();
        let local = body
            .split(" as ")
            .last()
            .unwrap_or(body)
            .split([':', '{', ','])
            .filter(|s| !s.trim().is_empty() && *s != "self" && *s != "super" && *s != "crate")
            .last()
            .unwrap_or(body)
            .trim()
            .to_string();
        return Some(Import {
            is_relative: body.starts_with("crate")
                || body.starts_with("self")
                || body.starts_with("super"),
            specifier: body.to_string(),
            local_name: Some(local),
            imported_name: None,
            line,
            kind: "use".to_string(),
            ..Default::default()
        });
    }
    // Java/Kotlin/Scala/C#/Swift.
    if matches!(lang, "java" | "kotlin" | "scala" | "csharp" | "swift") {
        let spec = raw
            .trim_start_matches("import ")
            .trim_start_matches("using ")
            .trim_end_matches(';')
            .trim()
            .to_string();
        let base = spec.rsplit('.').next().unwrap_or(&spec).to_string();
        return Some(Import {
            is_relative: false,
            specifier: spec,
            local_name: Some(base),
            imported_name: None,
            line,
            kind: "import".to_string(),
            ..Default::default()
        });
    }
    // JS/TS.
    if matches!(lang, "javascript" | "typescript" | "tsx" | "vue" | "svelte") {
        let spec = find_string_literal(node, source)?;
        let local = raw
            .split('{')
            .next()
            .and_then(|s| s.split_whitespace().nth(1))
            .filter(|s| *s != "from" && *s != "*")
            .map(|s| s.trim_matches(',').to_string());
        let spec = unquoted(&spec);
        return Some(Import {
            is_relative: spec.starts_with('.') || spec.starts_with('/'),
            specifier: spec,
            local_name: local,
            imported_name: None,
            line,
            kind: "import".to_string(),
            ..Default::default()
        });
    }
    // Generic: first quoted string or identifier.
    let spec = find_string_literal(node, source)
        .map(|s| unquoted(&s))
        .or_else(|| {
            let stripped = raw
                .trim_start_matches(|c: char| c.is_alphabetic() || c == '#')
                .trim_matches(|c: char| c == ':' || c == ';' || c.is_whitespace())
                .trim_matches(['<', '>', '"', '\''])
                .to_string();
            if stripped.is_empty() {
                None
            } else {
                Some(stripped)
            }
        })?;
    let local = spec.rsplit(['/', '.', ':']).next().map(|s| s.to_string());
    Some(Import {
        is_relative: spec.starts_with('.') || spec.starts_with('/'),
        specifier: spec,
        local_name: local,
        imported_name: None,
        line,
        kind: "import".to_string(),
        ..Default::default()
    })
}

fn find_string_literal(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if matches!(
            n.kind(),
            "string"
                | "string_literal"
                | "interpreted_string_literal"
                | "raw_string_literal"
                | "template_string"
                | "string_fragment"
        ) {
            return Some(text(source, n).to_string());
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

fn is_decorator_node(kind: &str) -> bool {
    kind.contains("decorator")
        || kind == "annotation"
        || kind == "marker_annotation"
        || kind == "attribute_item"
        || kind == "attribute_list"
}

fn preceding_decorators(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = node.prev_named_sibling();
    while let Some(sibling) = current {
        if is_decorator_node(sibling.kind()) {
            out.extend(decorator_names(sibling, source));
            current = sibling.prev_named_sibling();
        } else if sibling.kind() == "modifiers" {
            let mut cursor = sibling.walk();
            for child in sibling.named_children(&mut cursor) {
                if is_decorator_node(child.kind()) {
                    out.extend(decorator_names(child, source));
                }
            }
            break;
        } else {
            break;
        }
    }
    out.reverse();
    out
}

fn decorator_names(node: Node<'_>, source: &[u8]) -> Vec<String> {
    if !is_decorator_node(node.kind()) {
        return Vec::new();
    }
    let raw = text(source, node);
    let name = raw
        .trim()
        .trim_start_matches(['@', '#', '['])
        .trim_end_matches(']')
        .split(['(', ' ', '\n'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        Vec::new()
    } else {
        vec![name]
    }
}

fn preceding_docstring(source: &[u8], node: Node<'_>, lang: &str) -> Option<String> {
    let start = node.start_byte().min(source.len());
    // Only the recent window can contribute; converting the whole prefix for
    // every definition is quadratic on large files.
    let mut lo = start.saturating_sub(16 * 1024);
    if lo > 0 {
        if let Some(offset) = source[lo..start].iter().position(|b| *b == b'\n') {
            lo += offset + 1;
        }
    }
    let prefix = &source[lo..start];
    let text = String::from_utf8_lossy(prefix);
    let markers = comment_markers(lang);
    let mut lines: Vec<&str> = text.lines().collect();
    // Drop the incomplete current line.
    lines.pop();
    let mut collected: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in lines.iter().rev().take(60) {
        let trimmed = line.trim();
        if trimmed.is_empty() && collected.is_empty() {
            continue;
        }
        if trimmed.is_empty() {
            break;
        }
        if trimmed.ends_with("*/") {
            in_block = true;
        }
        let has_marker =
            markers.iter().any(|m| trimmed.starts_with(m)) || in_block || trimmed.starts_with('*');
        if !has_marker {
            break;
        }
        collected.push(clean_comment_line(trimmed));
        if trimmed.starts_with("/*") {
            break;
        }
    }
    collected.reverse();
    let doc = collected.join("\n").trim().to_string();
    if doc.is_empty() {
        None
    } else {
        Some(doc)
    }
}

fn clean_comment_line(line: &str) -> String {
    line.trim()
        .trim_start_matches("///")
        .trim_start_matches("//!")
        .trim_start_matches("//")
        .trim_start_matches("/**")
        .trim_start_matches("/*")
        .trim_start_matches("*/")
        .trim_start_matches('#')
        .trim_start_matches("--")
        .trim_start_matches('*')
        .trim()
        .to_string()
}

fn comment_markers(lang: &str) -> &'static [&'static str] {
    match lang {
        "python" | "ruby" | "bash" | "elixir" | "r" | "julia" | "nix" | "perl" | "powershell"
        | "make" | "dockerfile" | "yaml" | "toml" | "ini" | "dotenv" | "requirements" => &["#"],
        "haskell" | "lua" | "sql" | "ada" | "vhdl" => &["--"],
        "commonlisp" | "scheme" | "elisp" => &[";"],
        "fortran" | "cobol" => &["!", "c ", "C "],
        "matlab" => &["%"],
        _ => &["//", "/*", "*"],
    }
}

fn first_line_window(source: &[u8], node: Node<'_>, max: usize) -> String {
    let start = node.start_byte().min(source.len());
    let end = node.end_byte().min(source.len()).min(start + max * 4);
    let raw = String::from_utf8_lossy(&source[start..end]);
    first_line(&raw, max)
}

fn leading_comments(source: &[u8], _line: i32, markers: &[&str]) -> Option<String> {
    let text = String::from_utf8_lossy(source);
    let mut out = Vec::new();
    for line in text.lines().take(40) {
        let trimmed = line.trim();
        if trimmed.is_empty() && out.is_empty() {
            continue;
        }
        if trimmed.starts_with("#!") {
            continue;
        }
        if markers.iter().any(|m| trimmed.starts_with(m)) || trimmed.starts_with('*') {
            out.push(clean_comment_line(trimmed));
        } else {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n").trim().to_string())
    }
}

fn python_docstring(node: Node<'_>, source: &[u8]) -> Option<String> {
    let body = node.child_by_field_name("body")?;
    let first = body.named_child(0)?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let string = first.named_child(0)?;
    if string.kind() != "string" {
        return None;
    }
    Some(unquote_multiline(text(source, string)))
}

fn unquote_multiline(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw
        .trim_start_matches("\"\"\"")
        .trim_start_matches("'''")
        .trim_end_matches("\"\"\"")
        .trim_end_matches("'''");
    raw.trim().to_string()
}

fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    if (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''))
        || (raw.starts_with('`') && raw.ends_with('`'))
    {
        raw[1..raw.len().saturating_sub(1)].to_string()
    } else {
        raw.to_string()
    }
}

fn literal_string(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let unquoted = unquote(raw);
    if unquoted != raw
        || raw
            .chars()
            .all(|c| c.is_alphanumeric() || "_./-{}:".contains(c))
    {
        if unquoted.is_empty() {
            None
        } else {
            Some(unquoted)
        }
    } else {
        None
    }
}

fn url_prefix(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let quote = raw.chars().next()?;
    if quote != '\'' && quote != '"' && quote != '`' {
        return None;
    }
    let rest = &raw[1..];
    let end = rest.find(quote)?;
    let prefix = &rest[..end];
    if prefix.starts_with('/') || prefix.starts_with("http://") || prefix.starts_with("https://") {
        Some(prefix.to_string())
    } else {
        None
    }
}

fn split_http_url(url: &str) -> (Option<String>, String) {
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

fn is_http_client(call: &CallSite) -> bool {
    let receiver = call.receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    let name = call.name.to_ascii_lowercase();
    receiver.contains("requests")
        || receiver.contains("axios")
        || receiver.contains("http")
        || receiver.contains("fetch")
        || receiver.contains("client")
        || receiver.contains("curl")
        || matches!(
            name.as_str(),
            "get"
                | "post"
                | "put"
                | "patch"
                | "delete"
                | "request"
                | "fetch"
                | "getjson"
                | "postjson"
        )
}

fn is_spawn_callee(call: &CallSite) -> bool {
    let receiver = call.receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    let name = call.name.to_ascii_lowercase();
    let process_receiver = receiver.contains("subprocess")
        || receiver.contains("child_process")
        || receiver.contains("process")
        || receiver.contains("command")
        || receiver.contains("exec")
        || receiver.contains("shell");
    if process_receiver {
        return matches!(
            name.as_str(),
            "run"
                | "popen"
                | "spawn"
                | "spawnsync"
                | "execsync"
                | "exec"
                | "command"
                | "new"
                | "start"
        );
    }
    matches!(
        name.as_str(),
        "popen" | "spawnsync" | "execsync" | "posix_spawn" | "system" | "processstart"
    )
}

fn route_from_decorator(node: Node<'_>, source: &[u8], scope: &[String]) -> Option<RouteDef> {
    let raw = text(source, node).trim();
    let (method, path) = parse_route_text(raw)?;
    Some(RouteDef {
        method,
        path,
        handler: scope.last().cloned(),
        line: line(node),
        framework: "decorator".to_string(),
        scope: scope.to_vec(),
    })
}

fn route_from_call(call: &CallSite, _lang: &str) -> Option<RouteDef> {
    let path = call.args.first().and_then(|a| literal_string(a))?;
    if !path.starts_with('/') && !path.starts_with("http") {
        return None;
    }
    let receiver = call.receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    let name = call.name.to_ascii_lowercase();
    let routeish_receiver = receiver.contains("app")
        || receiver.contains("router")
        || receiver.contains("route")
        || receiver.contains("server")
        || receiver.contains("mux")
        || receiver.contains("api");
    let method = match name.as_str() {
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" => {
            Some(name.to_ascii_uppercase())
        }
        "handlefunc" | "handle" | "route" | "mapget" | "mappost" => Some("ANY".to_string()),
        _ => None,
    }?;
    if !routeish_receiver && name != "handlefunc" && name != "route" {
        return None;
    }
    let handler = call.args.get(1).map(|a| extract_callable_name(a));
    Some(RouteDef {
        method,
        path,
        handler,
        line: call.line,
        framework: receiver,
        scope: call.scope.clone(),
    })
}

fn parse_route_text(raw: &str) -> Option<(String, String)> {
    let lower = raw.to_ascii_lowercase();
    let method = if lower.contains("getmapping")
        || lower.contains(".get(")
        || lower.contains("@get(")
        || lower.contains("httpget")
    {
        "GET"
    } else if lower.contains("postmapping")
        || lower.contains(".post(")
        || lower.contains("@post(")
        || lower.contains("httppost")
    {
        "POST"
    } else if lower.contains("putmapping")
        || lower.contains(".put(")
        || lower.contains("@put(")
        || lower.contains("httpput")
    {
        "PUT"
    } else if lower.contains("deletemapping")
        || lower.contains(".delete(")
        || lower.contains("@delete(")
        || lower.contains("httpdelete")
    {
        "DELETE"
    } else if lower.contains("patchmapping")
        || lower.contains(".patch(")
        || lower.contains("@patch(")
    {
        "PATCH"
    } else if lower.contains("requestmapping")
        || lower.contains("@route(")
        || lower.contains(".route(")
    {
        "ANY"
    } else {
        return None;
    };
    let path = FILE_PATH_RE.captures(raw)?.get(1)?.as_str().to_string();
    Some((method.to_string(), path))
}

fn is_test_def(def: &Def, rel_path: &str, lang: &str) -> bool {
    if is_test_file(rel_path) {
        return true;
    }
    let name = def.name.to_ascii_lowercase();
    if name.starts_with("test_") || name.starts_with("test") && name.len() > 4 {
        return true;
    }
    if name.starts_with("it_") || name.starts_with("should_") {
        return true;
    }
    let decorations = def.decorators.join(" ").to_ascii_lowercase();
    if decorations.contains("test")
        || decorations.contains("fact")
        || decorations.contains("theory")
    {
        return true;
    }
    let _ = lang;
    false
}

fn is_test_file(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/__tests__/")
        || path.contains("/spec/")
        || path.ends_with("_test.go")
        || path.ends_with("_test.py")
        || path.ends_with("_test.rs")
        || path.ends_with(".test.ts")
        || path.ends_with(".test.tsx")
        || path.ends_with(".test.js")
        || path.ends_with(".spec.ts")
        || path.ends_with(".spec.js")
        || path.ends_with("_test.c")
        || path.ends_with("_test.cpp")
        || path.ends_with("test_main.c")
}

fn is_entry_point_name(name: &str, lang: &str) -> bool {
    match lang {
        "go" => name == "main" || name == "init",
        "java" | "csharp" => name == "main" || name == "Main",
        "rust" => name == "main",
        "c" | "cpp" | "cuda" => name == "main" || name == "_start" || name == "wWinMain",
        "python" => name == "__main__",
        "javascript" | "typescript" | "tsx" => name == "main",
        _ => matches!(name, "main" | "_start" | "init" | "constructor"),
    }
}

fn is_exported(node: Node<'_>, source: &[u8], lang: &str) -> bool {
    let raw = text(source, node);
    match lang {
        "rust" => raw.trim_start().starts_with("pub "),
        "javascript" | "typescript" | "tsx" => raw.trim_start().starts_with("export "),
        "python" => !raw.trim_start().starts_with('_'),
        "go" => node
            .child_by_field_name("name")
            .map(|n| {
                text(source, n)
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase())
            })
            .unwrap_or(false),
        "java" | "csharp" | "kotlin" | "swift" | "scala" => raw.contains("public "),
        _ => true,
    }
}

fn visibility(node: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = text(source, node);
    for word in [
        "pub(crate)",
        "public",
        "private",
        "protected",
        "internal",
        "pub",
    ] {
        if raw.trim_start().starts_with(word) || raw.contains(&format!(" {word} ")) {
            return Some(word.to_string());
        }
    }
    None
}

fn first_identifier_child<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut found = None;
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "field_identifier"
        ) {
            found = Some(child);
            break;
        }
    }
    found
}

fn first_type_node<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut found = None;
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "type_identifier" | "identifier" | "scoped_type_identifier" | "generic_type"
        ) {
            found = Some(child);
            break;
        }
    }
    found
}

fn collect_return_types(
    root: Node<'_>,
    source: &[u8],
    lang: &str,
    out: &mut HashMap<String, String>,
) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some((_, true)) = classify_definition(node, lang, None) {
            if let Some(name) = definition_name(node, source, lang) {
                if let Some(return_type) = extract_return_type(node, source) {
                    let simple = simple_type_name(&return_type);
                    if !simple.is_empty() {
                        out.entry(name).or_insert(simple);
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn collect_type_map(node: Node<'_>, source: &[u8], out: &mut HashMap<String, String>) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        let kind = n.kind();
        if matches!(
            kind,
            "variable_declarator"
                | "let_declaration"
                | "short_var_declaration"
                | "declaration"
                | "local_variable_declaration"
                | "property_declaration"
                | "assignment"
                | "init_declarator"
                | "typed_parameter"
                | "required_parameter"
                | "optional_parameter"
                | "formal_parameter"
                | "parameter_declaration"
                | "parameter"
                | "public_field_definition"
                | "field_declaration"
                | "field_definition"
                | "property_definition"
        ) {
            let name = n
                .child_by_field_name("name")
                .or_else(|| n.child_by_field_name("left"))
                .or_else(|| n.child_by_field_name("declarator"))
                .or_else(|| n.child_by_field_name("pattern"))
                .or_else(|| first_identifier_child(n))
                .and_then(|c| name_from_node(c, source));
            let type_name = n
                .child_by_field_name("type")
                .map(|c| simple_type_name(text(source, c)))
                .or_else(|| {
                    n.child_by_field_name("value").and_then(|v| {
                        if matches!(v.kind(), "new_expression" | "object_creation_expression") {
                            v.child_by_field_name("constructor")
                                .or_else(|| v.child_by_field_name("type"))
                                .map(|c| simple_type_name(text(source, c)))
                        } else {
                            None
                        }
                    })
                })
                .or_else(|| {
                    let value = n.child_by_field_name("value")?;
                    if matches!(value.kind(), "struct_expression" | "composite_literal") {
                        return value
                            .child_by_field_name("name")
                            .or_else(|| value.child_by_field_name("type"))
                            .or_else(|| first_type_node(value))
                            .map(|c| simple_type_name(text(source, c)));
                    }
                    if matches!(value.kind(), "call_expression" | "call") {
                        let f = value.child_by_field_name("function")?;
                        let raw = text(source, f);
                        if raw.ends_with("::new")
                            || raw.ends_with(".new")
                            || raw.ends_with("::default")
                        {
                            return raw
                                .rsplit_once("::")
                                .or_else(|| raw.rsplit_once('.'))
                                .map(|(prefix, _)| simple_type_name(prefix));
                        }
                    }
                    None
                })
                .or_else(|| {
                    // `let x = Type::new()`, `crate::Type { .. }`, `&Type`
                    let raw = text(source, n);
                    TYPE_ASSIGN_RE
                        .captures(raw)?
                        .get(1)
                        .map(|m| m.as_str().to_string())
                });
            if let (Some(name), Some(type_name)) = (name, type_name) {
                if !name.is_empty() && !type_name.is_empty() && name != type_name {
                    out.insert(name, type_name);
                }
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn collect_parse_gaps(root: Node<'_>, source: &[u8], out: &mut Vec<ParseGap>) {
    if !root.has_error() {
        return;
    }
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            let start = line(n);
            let end = line_end(n).max(start);
            if out.len() < 20 {
                out.push(ParseGap {
                    start_line: start,
                    end_line: end,
                    reason: if n.is_missing() {
                        "missing".to_string()
                    } else {
                        "error".to_string()
                    },
                });
            }
            continue;
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if child.has_error() || child.is_error() || child.is_missing() {
                stack.push(child);
            }
        }
    }
    let total_lines = count_lines(source).max(1) as i32;
    let covered: i32 = out
        .iter()
        .map(|g| (g.end_line - g.start_line + 1).max(1))
        .sum();
    if covered * 100 / total_lines >= 80 {
        out.clear();
        out.push(ParseGap {
            start_line: 1,
            end_line: total_lines,
            reason: "parse_unusable".to_string(),
        });
    }
}

fn extract_doc_refs(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    for cap in DOC_BACKTICK_RE.captures_iter(doc) {
        if let Some(m) = cap.get(1) {
            out.push(m.as_str().to_string());
        }
    }
    for cap in DOC_BRACKET_RE.captures_iter(doc) {
        if let Some(m) = cap.get(1) {
            out.push(m.as_str().to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn looks_like_file_path(raw: &str) -> bool {
    if raw.len() < 3 || raw.len() > 300 || raw.contains('\n') || raw.contains(' ') {
        return false;
    }
    let ext = raw.rsplit('.').next().unwrap_or("");
    !ext.is_empty()
        && ext.len() <= 8
        && matches!(
            ext,
            "md" | "rst"
                | "adoc"
                | "txt"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "cfg"
                | "conf"
                | "tf"
                | "hcl"
                | "sh"
                | "bash"
                | "py"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "go"
                | "rs"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "java"
                | "kt"
                | "cs"
                | "rb"
                | "php"
                | "swift"
                | "sql"
                | "proto"
                | "graphql"
                | "dockerfile"
                | "mk"
                | "csv"
                | "pdf"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_core::language::detect;

    fn extract(name: &str, source: &str) -> FileExtract {
        let lang = detect(name).unwrap();
        extract_file(name, lang, source.as_bytes())
    }

    #[test]
    fn rust_defs_calls_imports() {
        let src = r#"
use crate::parser::Parser;

pub struct Engine { count: usize }

impl Engine {
    /// Parse a request.
    pub fn parse(&mut self, input: &str) -> usize {
        let p = Parser::new();
        p.run(input)
    }
}

fn main() {
    let mut e = Engine::new();
    e.parse("x");
}
"#;
        let out = extract("src/lib.rs", src);
        assert!(out
            .defs
            .iter()
            .any(|d| d.name == "Engine" && d.label == "Struct"));
        let parse = out
            .defs
            .iter()
            .find(|d| d.name == "parse")
            .expect("parse method");
        assert_eq!(parse.label, "Method");
        assert_eq!(parse.scope, vec!["Engine"]);
        assert!(parse
            .docstring
            .as_deref()
            .unwrap()
            .contains("Parse a request"));
        assert!(out.calls.iter().any(|c| c.name == "run"));
        assert!(out.calls.iter().any(|c| c.name == "new"));
        assert!(out.imports.iter().any(|i| i.specifier.contains("parser")));
    }

    #[test]
    fn python_defs_and_docstring() {
        let src = r#"
class Service:
    """Service docs."""

    def handle(self, request):
        """Handle `request`."""
        return self.parse(request)

    def parse(self, request):
        return request
"#;
        let out = extract("app/service.py", src);
        let handle = out.defs.iter().find(|d| d.name == "handle").unwrap();
        assert_eq!(handle.label, "Method");
        assert_eq!(handle.scope, vec!["Service"]);
        assert!(handle.docstring.as_deref().unwrap().contains("Handle"));
        assert!(out.calls.iter().any(|c| c.name == "parse"));
    }

    #[test]
    fn js_imports_and_routes() {
        let src = r#"
import express from 'express';
const app = express();
app.get('/users/:id', getUser);
function getUser(req, res) {
  return fetch('/api/users/' + req.params.id);
}
"#;
        let out = extract("src/app.ts", src);
        assert!(out.imports.iter().any(|i| i.specifier == "express"));
        assert!(out
            .routes
            .iter()
            .any(|r| r.method == "GET" && r.path == "/users/:id"));
        assert!(out.http_calls.iter().any(|h| h.url_path == "/api/users/"));
    }

    #[test]
    fn c_functions_and_includes() {
        let src = r#"
#include "parser.h"
static int helper(int x) { return x + 1; }
int main(void) { return helper(1); }
"#;
        let out = extract("src/main.c", src);
        assert!(out.defs.iter().any(|d| d.name == "helper"));
        assert!(out
            .defs
            .iter()
            .any(|d| d.name == "main" && d.is_entry_point));
        assert!(out.imports.iter().any(|i| i.specifier == "parser.h"));
        assert!(out.calls.iter().any(|c| c.name == "helper"));
    }

    #[test]
    fn rust_receiver_type_inference() {
        let src = "pub struct Engine {}\nimpl Engine { pub fn parse(&self) {} }\nfn main() { let mut e = crate::Engine { }; e.parse(); }\n";
        let out = extract("src/lib.rs", src);
        let call = out
            .calls
            .iter()
            .find(|c| c.name == "parse")
            .expect("parse call");
        assert_eq!(call.receiver.as_deref(), Some("e"));
        assert_eq!(call.receiver_type.as_deref(), Some("Engine"));
    }

    #[test]
    fn rust_test_attribute_body_calls() {
        let src = "#[test]\nfn test_parse() { let mut e = crate::Engine { count: 0 }; assert_eq!(e.parse(\"abc\"), 3); }\n";
        let out = extract("tests/test_engine.rs", src);
        eprintln!(
            "defs={:?}",
            out.defs
                .iter()
                .map(|d| (&d.name, &d.label))
                .collect::<Vec<_>>()
        );
        eprintln!(
            "calls={:?}",
            out.calls
                .iter()
                .map(|c| (&c.name, &c.receiver, &c.receiver_type))
                .collect::<Vec<_>>()
        );
        assert!(out
            .calls
            .iter()
            .any(|c| c.name == "parse" && c.receiver_type.as_deref() == Some("Engine")));
    }

    #[test]
    fn parse_gaps_are_reported() {
        let out = extract("broken.rs", "fn main( {");
        assert!(!out.parse_gaps.is_empty());
    }
}
