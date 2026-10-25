//! Graph construction, relationship resolution and post-index passes.

use crate::discover::DiscoveredFile;
use crate::graph::{props, qn_file, qn_module, qn_symbol, GraphBuilder};
use crate::resolve::{caller_qn, import_edge_targets, Registry, RegistrySeed, Symbol};
use gry_core::fqn::{fqn_compute, fqn_folder};
use gry_core::minhash::Fingerprint;
use gry_core::nomic;
use gry_core::tokenize::{normalize_config_key, split_identifier, split_text};
use gry_core::types::{labels, NewNode};
use gry_parse::infra::{channel_qn, route_qn, FILE_PREFIX, FILE_SELF};
use gry_parse::model::{FileExtract, InfraNode};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub struct ParsedFile {
    pub discovered: DiscoveredFile,
    pub extract: FileExtract,
}

pub struct PartialInput {
    pub external_symbols: Vec<Symbol>,
    pub seed: RegistrySeed,
    pub file_qn: HashMap<String, String>,
    pub module_qn: HashMap<String, String>,
    pub external_qns: Vec<String>,
    /// Existing nodes (config keys, global infrastructure) needed by partial
    /// passes. They are upserted, not deleted.
    pub extra_nodes: Vec<NewNode>,
}

pub struct BuildOptions {
    pub project: String,
    pub root_path: String,
    pub mode: String,
    pub semantic_edges: bool,
    pub branch: Option<String>,
    pub working_tree: bool,
    pub partial: Option<PartialInput>,
    pub nodes_only: bool,
}

pub struct BuildResult {
    pub builder: GraphBuilder,
    pub registry: Registry,
    pub symbols: Vec<Symbol>,
}

pub fn build(parsed: &[ParsedFile], options: &BuildOptions) -> BuildResult {
    let phase_start = std::time::Instant::now();
    let mut builder = GraphBuilder::new(&options.project, &options.root_path, &options.mode);
    builder.add_project();
    if let Some(branch) = &options.branch {
        let branch_qn = format!("{}.__branch__.{branch}", options.project);
        builder.add_node(
            labels::BRANCH,
            branch,
            &branch_qn,
            "",
            0,
            0,
            props(&[("branch", branch)]),
        );
        builder.add_edge(&options.project, &branch_qn, "HAS_BRANCH", Map::new());
    }
    {
        let qn = format!("{}.__branch__.working-tree", options.project);
        builder.add_node(
            labels::BRANCH,
            "working-tree",
            &qn,
            "",
            0,
            0,
            props(&[
                ("branch", "working-tree"),
                (
                    "dirty",
                    if options.working_tree {
                        "true"
                    } else {
                        "false"
                    },
                ),
            ]),
        );
        builder.add_edge(&options.project, &qn, "HAS_BRANCH", Map::new());
    }
    if let Some(partial) = &options.partial {
        builder.seed_known(partial.file_qn.clone(), partial.module_qn.clone());
        builder
            .external_qns
            .extend(partial.external_qns.iter().cloned());
        for node in &partial.extra_nodes {
            builder.add_node(
                &node.label,
                &node.name,
                &node.qualified_name,
                &node.file_path,
                node.start_line,
                node.end_line,
                match &node.properties {
                    Value::Object(map) => map.clone(),
                    _ => Map::new(),
                },
            );
        }
    }
    let mut symbols: Vec<Symbol> = options
        .partial
        .as_ref()
        .map(|p| p.external_symbols.clone())
        .unwrap_or_default();
    let mut route_handlers: Vec<(String, String, String, Vec<String>)> = Vec::new();
    let mut openapi_handlers: Vec<(String, String)> = Vec::new();
    let mut adr_nodes: Vec<(String, String, Option<String>)> = Vec::new();

    // Pass 1: files, modules, definitions, sections, packages, infra nodes.
    for file in parsed {
        let rel = &file.discovered.rel_path;
        let file_qn = builder.file_node(rel, &file.discovered.language, file.discovered.size);
        builder.set_node_property(&file_qn, "lines", Value::Number(file.extract.lines.into()));
        if let Some(node) = builder.nodes.get_mut(&file_qn) {
            node.end_line = file.extract.lines as i32;
        }
        let module_qn = builder.module_node(
            rel,
            &file.discovered.language,
            file.discovered.module_is_dir,
        );
        let defs = &file.extract.defs;
        for def in defs {
            let qn = qn_symbol(&options.project, rel, &def.scope, &def.name);
            let mut props = def.properties.clone();
            if !def.params.is_empty() {
                props.insert(
                    "params".to_string(),
                    Value::Array(def.params.iter().cloned().map(Value::String).collect()),
                );
            }
            if let Some(return_type) = &def.return_type {
                props.insert(
                    "return_type".to_string(),
                    Value::String(return_type.clone()),
                );
            }
            if let Some(signature) = &def.signature {
                props.insert("signature".to_string(), Value::String(signature.clone()));
            }
            if !def.bases.is_empty() {
                props.insert(
                    "bases".to_string(),
                    Value::Array(
                        def.bases
                            .iter()
                            .map(|(name, imp)| serde_json::json!({"name": name, "implements": imp}))
                            .collect(),
                    ),
                );
            }
            if !def.decorators.is_empty() {
                props.insert(
                    "decorators".to_string(),
                    Value::Array(def.decorators.iter().cloned().map(Value::String).collect()),
                );
            }
            if let Some(complexity) = def.complexity {
                props.insert("complexity".to_string(), Value::Number(complexity.into()));
            }
            if let Some(cognitive) = def.cognitive {
                props.insert("cognitive".to_string(), Value::Number(cognitive.into()));
            }
            if let Some(depth) = def.loop_depth {
                props.insert("loop_depth".to_string(), Value::Number(depth.into()));
            }
            builder.add_node(
                &def.label,
                &def.name,
                &qn,
                rel,
                def.start_line,
                def.end_line,
                props,
            );
            builder.add_edge(&file_qn, &qn, "DEFINES", Map::new());
            if !def.scope.is_empty() {
                let parent_name = def.scope.join(".");
                let parent_qn = fqn_compute(&options.project, rel, Some(&parent_name));
                if builder.has_node(&parent_qn) {
                    builder.add_edge(&parent_qn, &qn, "DEFINES_METHOD", Map::new());
                }
            }
            symbols.push(Symbol {
                qn: qn.clone(),
                name: def.name.clone(),
                label: def.label.clone(),
                file_path: rel.clone(),
                language: file.discovered.language.clone(),
                module_qn: module_qn.clone(),
                scope: def.scope.clone(),
                start_line: def.start_line,
                is_test: def.is_test,
            });
        }

        // Documentation sections.
        add_sections(&mut builder, &options.project, rel, &file_qn, &file.extract);
        add_adr(
            &mut builder,
            &options.project,
            rel,
            &file_qn,
            &module_qn,
            &file.extract,
            &mut adr_nodes,
        );
        add_diagrams(&mut builder, &options.project, rel, &file_qn, &file.extract);

        // Config keys become Variable nodes.
        for key in &file.extract.config_keys {
            let key_name = format!("__config__{}", key.key);
            let qn = fqn_compute(&options.project, rel, Some(&key_name));
            let mut p = props(&[("config_key", &key.key), ("format", &key.format)]);
            if let Some(section) = &key.section {
                p.insert("section".to_string(), Value::String(section.clone()));
            }
            p.insert("line".to_string(), Value::Number(key.line.into()));
            builder.add_node(labels::VARIABLE, &key.key, &qn, rel, key.line, key.line, p);
            builder.add_edge(&file_qn, &qn, "DEFINES", Map::new());
        }

        // Package identity and dependencies.
        if let Some(package) = &file.extract.package {
            let package_qn = format!("__package__{}/{}", package.ecosystem, package.name);
            let mut p = props(&[
                ("name", &package.name),
                ("ecosystem", &package.ecosystem),
                ("manifest", rel),
            ]);
            if let Some(version) = &package.version {
                p.insert("version".to_string(), Value::String(version.clone()));
            }
            if !builder.has_node(&package_qn) {
                builder.add_node(labels::PACKAGE, &package.name, &package_qn, rel, 0, 0, p);
            }
            builder.add_edge(&file_qn, &package_qn, "DEFINES", Map::new());
            for dep in &package.dependencies {
                let dep_qn = format!("__package__{}/{}", package.ecosystem, dep);
                if !builder.has_node(&dep_qn) {
                    builder.add_node(
                        labels::PACKAGE,
                        dep,
                        &dep_qn,
                        "",
                        0,
                        0,
                        props(&[("ecosystem", &package.ecosystem), ("external", "true")]),
                    );
                }
                builder.add_edge(
                    &file_qn,
                    &dep_qn,
                    "DEPENDS_ON",
                    props(&[("via", "manifest")]),
                );
            }
        }

        // Routes declared in code.
        for route in &file.extract.routes {
            let qn = route_qn(&route.method, &route.path);
            let mut p = props(&[
                ("method", &route.method),
                ("url_path", &route.path),
                ("source", "decorator"),
            ]);
            if !route.framework.is_empty() {
                p.insert(
                    "framework".to_string(),
                    Value::String(route.framework.clone()),
                );
            }
            builder.add_node(
                labels::ROUTE,
                &route.path,
                &qn,
                rel,
                route.line,
                route.line,
                p,
            );
            let handler = route
                .handler
                .clone()
                .or_else(|| route.scope.last().cloned())
                .unwrap_or_default();
            route_handlers.push((rel.clone(), handler, qn, route.scope.clone()));
        }

        // OpenAPI operations.
        for op in &file.extract.openapi {
            let qn = route_qn(&op.method, &op.path);
            let mut p = props(&[
                ("method", &op.method),
                ("url_path", &op.path),
                ("source", "openapi"),
            ]);
            if let Some(op_id) = &op.operation_id {
                p.insert("operation_id".to_string(), Value::String(op_id.clone()));
            }
            if let Some(summary) = &op.summary {
                p.insert("summary".to_string(), Value::String(summary.clone()));
            }
            builder.add_node(labels::ROUTE, &op.path, &qn, rel, op.line, op.line, p);
            if let Some(op_id) = &op.operation_id {
                openapi_handlers.push((op_id.clone(), qn));
            }
        }

        // Infra nodes and raw edges.
        for node in &file.extract.infra_nodes {
            add_infra_node(&mut builder, node);
            if node.label == "EnvVar"
                && node.properties.get("source").and_then(Value::as_str) == Some("dockerfile")
            {
                let var_qn = fqn_compute(&options.project, rel, Some(&node.name));
                builder.add_node(
                    labels::VARIABLE,
                    &node.name,
                    &var_qn,
                    rel,
                    node.start_line,
                    node.start_line,
                    props(&[("env", &node.name)]),
                );
                builder.add_edge(&file_qn, &var_qn, "DEFINES", Map::new());
            }
        }
        // Env / channel / process / HTTP nodes from code.
        for env in &file.extract.env_accesses {
            let qn = format!("__env__{}", env.key);
            if !builder.has_node(&qn) {
                builder.add_node(
                    labels::ENV_VAR,
                    &env.key,
                    &qn,
                    "",
                    0,
                    0,
                    props(&[("name", &env.key)]),
                );
            }
            let caller = scope_qn(
                &options.project,
                rel,
                &env.scope,
                file.discovered.module_is_dir,
                &module_qn,
            );
            if builder.has_node(&caller) {
                builder.add_edge(
                    &caller,
                    &qn,
                    "CONFIGURES",
                    props(&[("strategy", "env_access"), ("mode", &env.mode)]),
                );
            }
        }
        for channel in &file.extract.channels {
            let qn = channel_qn(&channel.name);
            if !builder.has_node(&qn) {
                builder.add_node(
                    labels::CHANNEL,
                    &channel.name,
                    &qn,
                    "",
                    0,
                    0,
                    props(&[("broker", &channel.broker)]),
                );
            }
            let caller = scope_qn(
                &options.project,
                rel,
                &channel.scope,
                file.discovered.module_is_dir,
                &module_qn,
            );
            if builder.has_node(&caller) {
                let edge_type = if channel.action == "emit" {
                    "EMITS"
                } else {
                    "LISTENS_ON"
                };
                builder.add_edge(
                    &caller,
                    &qn,
                    edge_type,
                    props(&[("broker", &channel.broker)]),
                );
            }
        }
        for spawn in &file.extract.spawns {
            let qn = format!("__process__{}", spawn.program);
            if !builder.has_node(&qn) {
                builder.add_node(
                    labels::PROCESS,
                    &spawn.program,
                    &qn,
                    "",
                    0,
                    0,
                    props(&[("dynamic", if spawn.dynamic { "true" } else { "false" })]),
                );
            }
            let caller = scope_qn(
                &options.project,
                rel,
                &spawn.scope,
                file.discovered.module_is_dir,
                &module_qn,
            );
            if builder.has_node(&caller) {
                builder.add_edge(&caller, &qn, "SPAWNS", props(&[("via", "code")]));
            }
        }
        for http in &file.extract.http_calls {
            let qn = route_qn(&http.method, &http.url_path);
            if !builder.has_node(&qn) {
                let mut p = props(&[
                    ("method", &http.method),
                    ("url_path", &http.url_path),
                    ("source", "client"),
                ]);
                if let Some(host) = &http.host {
                    p.insert("host".to_string(), Value::String(host.clone()));
                }
                builder.add_node(
                    labels::ROUTE,
                    &http.url_path,
                    &qn,
                    rel,
                    http.line,
                    http.line,
                    p,
                );
            }
            let caller = scope_qn(
                &options.project,
                rel,
                &http.scope,
                file.discovered.module_is_dir,
                &module_qn,
            );
            if builder.has_node(&caller) {
                let mut p = props(&[("method", &http.method), ("url_path", &http.url_path)]);
                if let Some(host) = &http.host {
                    p.insert("host".to_string(), Value::String(host.clone()));
                }
                builder.add_edge(&caller, &qn, "HTTP_CALLS", p);
            }
        }
    }

    crate::phase!("  defs/docs/keys/infra", phase_start);
    if options.nodes_only {
        return BuildResult {
            builder,
            registry: Registry::default(),
            symbols,
        };
    }
    let phase_start = std::time::Instant::now();
    // Pass 2: resolve imports and add external package nodes.
    let file_tuples: Vec<(FileExtract, String, bool)> = parsed
        .iter()
        .map(|f| {
            (
                f.extract.clone(),
                f.discovered.rel_path.clone(),
                f.discovered.module_is_dir,
            )
        })
        .collect();
    let empty_seed = RegistrySeed::default();
    let seed = options
        .partial
        .as_ref()
        .map(|p| &p.seed)
        .unwrap_or(&empty_seed);
    let registry = Registry::build_with_seed(&options.project, &file_tuples, symbols.clone(), seed);
    let symbols = registry.symbols.clone();

    for file in parsed {
        let rel = &file.discovered.rel_path;
        let module_qn = qn_module(&options.project, rel, file.discovered.module_is_dir);
        for import in &file.extract.imports {
            let (target_module, external_pkg) = import_edge_targets(
                &options.project,
                rel,
                file.discovered.module_is_dir,
                &registry,
                import,
            );
            let mut p = props(&[
                ("specifier", &import.specifier),
                ("line", &import.line.to_string()),
                ("kind", &import.kind),
            ]);
            if let Some(local) = &import.local_name {
                p.insert("local_name".to_string(), Value::String(local.clone()));
            }
            if let Some(target) = target_module {
                if builder.has_node_or_external(&target) {
                    builder.add_edge(&module_qn, &target, "IMPORTS", p);
                }
            } else if let Some(pkg) = external_pkg {
                let ecosystem = ecosystem_for_language(&file.discovered.language);
                let pkg_qn = format!("__package__{ecosystem}/{pkg}");
                if !builder.has_node(&pkg_qn) {
                    builder.add_node(
                        labels::PACKAGE,
                        &pkg,
                        &pkg_qn,
                        "",
                        0,
                        0,
                        props(&[("ecosystem", ecosystem), ("external", "true")]),
                    );
                }
                builder.add_edge(&module_qn, &pkg_qn, "IMPORTS", p);
            }
        }
    }

    crate::phase!("  registry+imports", phase_start);
    let phase_start = std::time::Instant::now();
    // Pass 3: resolve calls, usages, inheritance, decorators, docs, routes.
    let mut call_names_by_caller: HashSet<(String, String)> = HashSet::new();
    let test_qns: HashSet<&str> = symbols
        .iter()
        .filter(|s| s.is_test)
        .map(|s| s.qn.as_str())
        .collect();
    for file in parsed {
        let rel = &file.discovered.rel_path;
        let module_qn = qn_module(&options.project, rel, file.discovered.module_is_dir);
        for call in &file.extract.calls {
            let caller = scope_qn(
                &options.project,
                rel,
                &call.scope,
                file.discovered.module_is_dir,
                &module_qn,
            );
            let caller = if builder.has_node(&caller) {
                caller
            } else {
                module_qn.clone()
            };
            call_names_by_caller.insert((caller.clone(), call.name.clone()));
            if let Some(resolved) = registry.resolve_call(
                &options.project,
                rel,
                &module_qn,
                &call.scope,
                &file.discovered.language,
                call,
            ) {
                let mut p = props(&[
                    ("confidence", &format!("{:.2}", resolved.confidence)),
                    ("resolved_by", resolved.resolved_by),
                    ("line", &call.line.to_string()),
                    ("callee", &call.name),
                ]);
                if resolved.ambiguous {
                    p.insert("ambiguous".to_string(), Value::Bool(true));
                }
                if call.args.len() <= 32 && !call.args.is_empty() {
                    let args: Vec<Value> = call
                        .args
                        .iter()
                        .take(8)
                        .map(|a| Value::String(a.chars().take(120).collect()))
                        .collect();
                    p.insert("callee_args".to_string(), Value::Array(args));
                }
                if let Some(receiver) = &call.receiver {
                    p.insert("receiver".to_string(), Value::String(receiver.clone()));
                }
                if let Some(receiver_type) = &call.receiver_type {
                    p.insert(
                        "receiver_type".to_string(),
                        Value::String(receiver_type.clone()),
                    );
                }
                builder.add_edge(&caller, &resolved.target_qn, "CALLS", p.clone());
                // Approximate argument-to-parameter data flow.
                let target_has_params = builder
                    .nodes
                    .get(&resolved.target_qn)
                    .and_then(|n| n.properties.get("params"))
                    .and_then(Value::as_array)
                    .map(|p| !p.is_empty())
                    .unwrap_or(false);
                if !call.args.is_empty() && target_has_params {
                    let mut flow = props(&[("line", &call.line.to_string())]);
                    flow.insert(
                        "args".to_string(),
                        Value::Array(
                            call.args
                                .iter()
                                .take(8)
                                .map(|a| Value::String(a.chars().take(120).collect()))
                                .collect(),
                        ),
                    );
                    flow.insert(
                        "mapping".to_string(),
                        Value::String("positional".to_string()),
                    );
                    builder.add_edge(&caller, &resolved.target_qn, "DATA_FLOWS", flow);
                }
                // Test reachability.
                let caller_is_test = test_qns.contains(caller.as_str());
                let target_is_test = test_qns.contains(resolved.target_qn.as_str());
                if caller_is_test && !target_is_test {
                    builder.add_edge(
                        &caller,
                        &resolved.target_qn,
                        "TESTS",
                        props(&[("line", &call.line.to_string())]),
                    );
                }
            }
        }
        // Usages (doc references and parser-detected identifier references).
        for usage in &file.extract.usages {
            let caller = scope_qn(
                &options.project,
                rel,
                &usage.scope,
                file.discovered.module_is_dir,
                &module_qn,
            );
            let caller = if builder.has_node(&caller) {
                caller
            } else {
                module_qn.clone()
            };
            if call_names_by_caller.contains(&(caller.clone(), usage.name.clone())) {
                continue;
            }
            if let Some(resolved) = registry.resolve_usage(rel, &file.discovered.language, usage) {
                if resolved.target_qn == caller {
                    continue;
                }
                let edge_type = if usage.kind == "doc_ref" {
                    "MENTIONS"
                } else {
                    "USAGE"
                };
                let mut p = props(&[
                    ("confidence", &format!("{:.2}", resolved.confidence)),
                    ("resolved_by", resolved.resolved_by),
                    ("line", &usage.line.to_string()),
                ]);
                if usage.kind == "doc_ref" {
                    p.insert("via".to_string(), Value::String("docstring".to_string()));
                    p.insert("tier".to_string(), Value::String("unique".to_string()));
                }
                builder.add_edge(&caller, &resolved.target_qn, edge_type, p);
            }
        }
    }

    crate::phase!("  calls+usages", phase_start);
    let phase_start = std::time::Instant::now();
    // Inheritance / implementation / override / decorators.
    for file in parsed {
        let rel = &file.discovered.rel_path;
        let module_qn = qn_module(&options.project, rel, file.discovered.module_is_dir);
        for def in &file.extract.defs {
            let qn = qn_symbol(&options.project, rel, &def.scope, &def.name);
            for (base, implements) in &def.bases {
                if let Some(resolved) = registry.resolve_type(rel, &file.discovered.language, base)
                {
                    let edge_type = if *implements {
                        "IMPLEMENTS"
                    } else {
                        "INHERITS"
                    };
                    builder.add_edge(
                        &qn,
                        &resolved.target_qn,
                        edge_type,
                        props(&[
                            ("confidence", &format!("{:.2}", resolved.confidence)),
                            ("resolved_by", resolved.resolved_by),
                        ]),
                    );
                }
            }
            for decorator in &def.decorators {
                let dummy = gry_parse::model::CallSite {
                    name: decorator
                        .rsplit(['.', ':'])
                        .next()
                        .unwrap_or(decorator)
                        .to_string(),
                    ..Default::default()
                };
                if let Some(resolved) = registry.resolve_call(
                    &options.project,
                    rel,
                    &module_qn,
                    &def.scope,
                    &file.discovered.language,
                    &dummy,
                ) {
                    builder.add_edge(
                        &qn,
                        &resolved.target_qn,
                        "DECORATES",
                        props(&[
                            ("confidence", &format!("{:.2}", resolved.confidence)),
                            ("decorator", decorator),
                        ]),
                    );
                } else {
                    let deco_qn = format!("<decorator:{decorator}>");
                    if !builder.has_node(&deco_qn) {
                        builder.add_node(
                            labels::DECORATOR,
                            decorator,
                            &deco_qn,
                            rel,
                            def.start_line,
                            def.start_line,
                            Map::new(),
                        );
                    }
                    builder.add_edge(
                        &qn,
                        &deco_qn,
                        "DECORATES",
                        props(&[("decorator", decorator)]),
                    );
                }
            }
        }
    }
    // OVERRIDE edges: a derived method overriding a base method with the same name.
    for file in parsed {
        let rel = &file.discovered.rel_path;
        for def in &file.extract.defs {
            if def.label != labels::METHOD || def.scope.is_empty() {
                continue;
            }
            let derived_qn = qn_symbol(&options.project, rel, &def.scope, &def.name);
            // Find the class's INHERITS targets.
            let class_qn = fqn_compute(&options.project, rel, Some(&def.scope.join(".")));
            let bases: Vec<String> = builder
                .edges
                .values()
                .filter(|e| e.source_qn == class_qn && e.edge_type == "INHERITS")
                .map(|e| e.target_qn.clone())
                .collect();
            for base in bases {
                if let Some(base_symbol) = symbols
                    .iter()
                    .find(|s| s.qn.starts_with(&format!("{base}.")) && s.name == def.name)
                {
                    builder.add_edge(
                        &derived_qn,
                        &base_symbol.qn,
                        "OVERRIDE",
                        props(&[("via", "inheritance")]),
                    );
                }
            }
        }
    }

    crate::phase!("  inheritance/decorators", phase_start);
    let phase_start = std::time::Instant::now();
    // Route handlers and OpenAPI operation handlers.
    for (rel, handler, route_qn, scope) in route_handlers {
        let module_qn = parsed
            .iter()
            .find(|f| f.discovered.rel_path == rel)
            .map(|f| qn_module(&options.project, &rel, f.discovered.module_is_dir))
            .unwrap_or_else(|| options.project.clone());
        let mut target = None;
        if !handler.is_empty() {
            target = symbols
                .iter()
                .find(|s| s.file_path == rel && s.name == handler)
                .map(|s| s.qn.clone());
        }
        if target.is_none() && !scope.is_empty() {
            let qn = qn_symbol(
                &options.project,
                &rel,
                &scope[..scope.len().saturating_sub(1)],
                scope.last().unwrap(),
            );
            if builder.has_node(&qn) {
                target = Some(qn);
            }
        }
        if let Some(target) = target {
            builder.add_edge(&target, &route_qn, "HANDLES", props(&[("via", "route")]));
        } else {
            let _ = module_qn;
        }
    }
    for (operation_id, route_qn) in openapi_handlers {
        if let Some(symbol) = symbols.iter().find(|s| s.name == operation_id) {
            builder.add_edge(
                &symbol.qn,
                &route_qn,
                "HANDLES",
                props(&[("via", "openapi")]),
            );
        }
    }

    crate::phase!("  routes/openapi", phase_start);
    let phase_start = std::time::Instant::now();
    // Section doc references → MENTIONS edges.
    let section_refs: Vec<(String, Vec<String>)> = builder
        .nodes
        .iter()
        .filter(|(_, n)| n.label == labels::SECTION)
        .filter_map(|(qn, n)| {
            let refs = n.properties.get("doc_refs")?.as_array()?;
            Some((
                qn.clone(),
                refs.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            ))
        })
        .collect();
    for (section_qn, refs) in section_refs {
        for raw in refs {
            let usage = gry_parse::model::Usage {
                name: raw.clone(),
                line: 0,
                kind: "doc_ref".to_string(),
                ..Default::default()
            };
            let file = builder
                .nodes
                .get(&section_qn)
                .map(|n| n.file_path.clone())
                .unwrap_or_default();
            let language = builder
                .nodes
                .get(&section_qn)
                .and_then(|n| n.properties.get("language"))
                .and_then(Value::as_str)
                .unwrap_or("markdown");
            if let Some(resolved) = registry.resolve_usage(&file, language, &usage) {
                builder.add_edge(
                    &section_qn,
                    &resolved.target_qn,
                    "MENTIONS",
                    props(&[
                        ("via", "doc"),
                        ("tier", "unique"),
                        ("confidence", &format!("{:.2}", resolved.confidence)),
                    ]),
                );
            }
        }
    }

    // ADR supersession.
    for (adr_qn, _id, superseded_by) in &adr_nodes {
        if let Some(by) = superseded_by {
            if let Some(target) = registry.resolve_adr(by) {
                builder.add_edge(
                    adr_qn,
                    target,
                    "MENTIONS",
                    props(&[("via", "superseded_by"), ("tier", "unique")]),
                );
            }
        }
    }
    // Supersedes statements parsed on each ADR.
    for file in parsed {
        if let Some(adr) = &file.extract.adr {
            let module_qn = qn_module(
                &options.project,
                &file.discovered.rel_path,
                file.discovered.module_is_dir,
            );
            let adr_qn = format!("{module_qn}.__adr__");
            for target_id in &adr.supersedes {
                if let Some(target) = registry.resolve_adr(target_id) {
                    if *target != adr_qn {
                        builder.add_edge(
                            &adr_qn,
                            target,
                            "SUPERSEDES",
                            props(&[("via", "adr"), ("statement", target_id)]),
                        );
                    }
                }
            }
        }
    }

    // Pass 4: infra edges with placeholder resolution.
    for file in parsed {
        for edge in &file.extract.infra_edges {
            let source = resolve_infra_qn(
                &mut builder,
                &options.project,
                &file.discovered.rel_path,
                &edge.source_qn,
            );
            let target = resolve_infra_qn(
                &mut builder,
                &options.project,
                &file.discovered.rel_path,
                &edge.target_qn,
            );
            if let (Some(source), Some(target)) = (source, target) {
                builder.add_edge(&source, &target, &edge.edge_type, edge.properties.clone());
            }
        }
    }

    crate::phase!("  sections/adr/infra", phase_start);
    let phase_start = std::time::Instant::now();
    // Pass 5: file references (docs and string literals).
    for file in parsed {
        let source_file = qn_file(&options.project, &file.discovered.rel_path);
        for reference in &file.extract.file_refs {
            let target_rel = resolve_repo_path(&file.discovered.rel_path, &reference.path);
            if let Some(target) = builder.file_qn.get(&target_rel).cloned() {
                builder.add_edge(
                    &source_file,
                    &target,
                    "REFERENCES_FILE",
                    props(&[
                        ("line", &reference.line.to_string()),
                        ("raw", &reference.path),
                    ]),
                );
            } else {
                let folder_qn = fqn_folder(&options.project, target_rel.trim_end_matches('/'));
                if builder.has_node(&folder_qn) {
                    builder.add_edge(
                        &source_file,
                        &folder_qn,
                        "REFERENCES_FILE",
                        props(&[
                            ("line", &reference.line.to_string()),
                            ("raw", &reference.path),
                        ]),
                    );
                }
            }
        }
    }

    // Pass 6: config key → symbol linking.
    config_links(&mut builder, parsed, &symbols);

    crate::phase!("  file refs + config", phase_start);
    let phase_start = std::time::Instant::now();
    // Pass 7: semantic edges and vectors.
    let partial_files: Option<HashSet<&str>> = options.partial.as_ref().map(|_| {
        parsed
            .iter()
            .map(|f| f.discovered.rel_path.as_str())
            .collect()
    });
    attach_vectors(
        &mut builder,
        parsed,
        &symbols,
        &options.project,
        partial_files.as_ref(),
    );
    if options.semantic_edges && options.partial.is_none() {
        similarity_edges(&mut builder);
        semantic_related_edges(&mut builder, &symbols);
    }
    crate::phase!("  vectors/semantic", phase_start);
    if options.partial.is_none() {
        let phase_start = std::time::Instant::now();
        importance(&mut builder, &symbols);
        crate::phase!("  importance", phase_start);
    }

    BuildResult {
        builder,
        registry,
        symbols,
    }
}

fn add_infra_node(builder: &mut GraphBuilder, node: &InfraNode) {
    builder.add_node(
        &node.label,
        &node.name,
        &node.qualified_name,
        &node.file_path,
        node.start_line,
        node.end_line,
        node.properties.clone(),
    );
}

fn add_sections(
    builder: &mut GraphBuilder,
    project: &str,
    rel: &str,
    file_qn: &str,
    extract: &FileExtract,
) {
    let mut stack: Vec<(u32, String)> = Vec::new();
    for section in &extract.sections {
        let qn = fqn_compute(project, rel, Some(&section.title));
        let mut p = Map::new();
        p.insert("level".to_string(), Value::Number(section.level.into()));
        p.insert("title".to_string(), Value::String(section.title.clone()));
        p.insert(
            "docstring".to_string(),
            Value::String(section.body.chars().take(4000).collect()),
        );
        if let Some(anchor) = &section.anchor {
            p.insert("anchor".to_string(), Value::String(anchor.clone()));
        }
        builder.add_node(
            labels::SECTION,
            &section.title,
            &qn,
            rel,
            section.start_line,
            section.end_line,
            p,
        );
        while stack
            .last()
            .map(|(l, _)| *l >= section.level)
            .unwrap_or(false)
        {
            stack.pop();
        }
        if let Some((_, parent_qn)) = stack.last() {
            builder.add_edge(parent_qn, &qn, "DEFINES", Map::new());
        } else {
            builder.add_edge(file_qn, &qn, "DEFINES", Map::new());
        }
        stack.push((section.level, qn.clone()));
    }
    // Section doc references: resolved later against symbols by the query layer.
    for reference in &extract.doc_refs {
        if reference.target_kind != "code" {
            continue;
        }
        let target = extract
            .sections
            .iter()
            .find(|s| reference.line >= s.start_line && reference.line <= s.end_line);
        if let Some(section) = target {
            let section_qn = fqn_compute(project, rel, Some(&section.title));
            // Stored as a property so the resolver can add MENTIONS after the
            // symbol registry exists.
            if let Some(node) = builder.nodes.get_mut(&section_qn) {
                if let Value::Object(ref mut p) = node.properties {
                    let entry = p
                        .entry("doc_refs".to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Value::Array(items) = entry {
                        items.push(Value::String(reference.raw.clone()));
                    }
                }
            }
        }
    }
}

fn add_adr(
    builder: &mut GraphBuilder,
    _project: &str,
    rel: &str,
    file_qn: &str,
    module_qn: &str,
    extract: &FileExtract,
    out: &mut Vec<(String, String, Option<String>)>,
) {
    let Some(adr) = &extract.adr else {
        return;
    };
    let qn = format!("{module_qn}.__adr__");
    let mut p = props(&[("adr_id", &adr.id), ("title", &adr.title)]);
    if let Some(status) = &adr.status {
        p.insert("status".to_string(), Value::String(status.clone()));
    }
    if let Some(date) = &adr.date {
        p.insert("date".to_string(), Value::String(date.clone()));
    }
    if !adr.deciders.is_empty() {
        p.insert(
            "deciders".to_string(),
            Value::Array(adr.deciders.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(by) = &adr.superseded_by {
        p.insert("superseded_by".to_string(), Value::String(by.clone()));
    }
    if let Some(doc) = &adr.docstring {
        p.insert("docstring".to_string(), Value::String(doc.clone()));
    }
    builder.add_node(labels::ADR, &adr.title, &qn, rel, 0, 0, p);
    builder.add_edge(file_qn, &qn, "DEFINES", Map::new());
    out.push((qn, adr.id.clone(), adr.superseded_by.clone()));
}

fn add_diagrams(
    builder: &mut GraphBuilder,
    project: &str,
    rel: &str,
    file_qn: &str,
    extract: &FileExtract,
) {
    for diagram in &extract.diagrams {
        let diagram_qn = fqn_compute(
            project,
            rel,
            Some(&format!("__diagram__{}", slugify(&diagram.name))),
        );
        let mut p = props(&[("kind", &diagram.kind), ("line", &diagram.line.to_string())]);
        p.insert("docstring".to_string(), Value::String(diagram.name.clone()));
        builder.add_node(
            labels::DIAGRAM,
            &diagram.name,
            &diagram_qn,
            rel,
            diagram.line,
            diagram.line,
            p,
        );
        builder.add_edge(file_qn, &diagram_qn, "DEFINES", Map::new());
        for element in &diagram.elements {
            let element_qn = format!("{diagram_qn}.{}", element.id);
            builder.add_node(
                labels::DIAGRAM_ELEMENT,
                &element.label,
                &element_qn,
                rel,
                element.line,
                element.line,
                props(&[("element_id", &element.id)]),
            );
            builder.add_edge(&diagram_qn, &element_qn, "DEFINES", Map::new());
        }
        for arrow in &diagram.arrows {
            let source = format!("{diagram_qn}.{}", arrow.source);
            let target = format!("{diagram_qn}.{}", arrow.target);
            if builder.has_node(&source) && builder.has_node(&target) {
                let mut p = Map::new();
                if let Some(label) = &arrow.label {
                    p.insert("label".to_string(), Value::String(label.clone()));
                }
                builder.add_edge(&source, &target, "DIAGRAM_ARROW", p);
            }
        }
    }
}

fn scope_qn(
    project: &str,
    rel: &str,
    scope: &[String],
    module_is_dir: bool,
    module_qn: &str,
) -> String {
    if scope.is_empty() {
        module_qn.to_string()
    } else {
        caller_qn(project, rel, scope, module_is_dir)
    }
}

fn resolve_infra_qn(
    builder: &mut GraphBuilder,
    project: &str,
    rel: &str,
    qn: &str,
) -> Option<String> {
    if qn == FILE_SELF {
        return builder.module_for(rel).cloned();
    }
    if let Some(path) = qn.strip_prefix(FILE_PREFIX) {
        let path = path.trim_start_matches('/');
        if let Some(file_qn) = builder.file_for(path) {
            return Some(file_qn.clone());
        }
        // A directory reference (Terraform module, Ansible role).
        let folder_qn = fqn_folder(project, path.trim_end_matches('/'));
        if builder.has_node(&folder_qn) {
            return Some(folder_qn);
        }
        // Create a placeholder File node so the relationship is not lost.
        let file_qn = qn_file(project, path);
        let name = path.rsplit('/').next().unwrap_or(path);
        builder.add_node(
            labels::FILE,
            name,
            &file_qn,
            path,
            0,
            0,
            props(&[("placeholder", "true")]),
        );
        return Some(file_qn);
    }
    if qn == "@chart" {
        // The chart node in this file/dir, if any.
        let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for (candidate, node) in &builder.nodes {
            if node.label == labels::CHART
                && (node.file_path == rel
                    || (!dir.is_empty() && node.file_path.starts_with(&format!("{dir}/"))))
            {
                return Some(candidate.clone());
            }
        }
        return None;
    }
    if let Some(rest) = qn.strip_prefix("__k8s_svc__") {
        let (ns, name) = rest.split_once('/').unwrap_or(("-", rest));
        for (candidate, node) in &builder.nodes {
            if node.label == labels::RESOURCE
                && node.name == name
                && node
                    .properties
                    .get("namespace")
                    .and_then(Value::as_str)
                    .map(|n| n == ns)
                    .unwrap_or(ns == "-")
                && node.properties.get("kind").and_then(Value::as_str) == Some("Service")
            {
                return Some(candidate.clone());
            }
        }
        return None;
    }
    Some(qn.to_string())
}

fn resolve_repo_path(from_file: &str, raw: &str) -> String {
    let raw = raw.split('#').next().unwrap_or(raw).trim();
    let raw = raw.trim_start_matches('/');
    if raw.starts_with("../") || raw.starts_with("./") {
        let base = from_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let mut stack: Vec<&str> = if base.is_empty() {
            Vec::new()
        } else {
            base.split('/').collect()
        };
        for part in raw.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    stack.pop();
                }
                p => stack.push(p),
            }
        }
        stack.join("/")
    } else {
        raw.to_string()
    }
}

fn ecosystem_for_language(language: &str) -> &'static str {
    match language {
        "javascript" | "typescript" | "tsx" | "vue" | "svelte" => "npm",
        "python" => "pypi",
        "rust" => "cargo",
        "go" => "go",
        "java" => "maven",
        "kotlin" | "scala" => "jvm",
        "csharp" | "fsharp" => "nuget",
        "ruby" => "rubygems",
        "php" => "composer",
        "swift" => "swiftpm",
        "dart" => "pub",
        "elixir" => "hex",
        _ => "unknown",
    }
}

fn attach_vectors(
    builder: &mut GraphBuilder,
    parsed: &[ParsedFile],
    symbols: &[Symbol],
    project: &str,
    only_files: Option<&HashSet<&str>>,
) {
    let mut callees: HashMap<String, Vec<String>> = HashMap::new();
    for edge in builder.edges.values() {
        if edge.edge_type == "CALLS" {
            let name = edge
                .properties
                .get("callee")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !name.is_empty() {
                callees
                    .entry(edge.source_qn.clone())
                    .or_default()
                    .push(name.to_string());
            }
        }
    }
    let mut docstrings: HashMap<String, String> = HashMap::new();
    for file in parsed {
        for def in &file.extract.defs {
            if let Some(doc) = &def.docstring {
                let qn = qn_symbol(project, &file.discovered.rel_path, &def.scope, &def.name);
                docstrings.insert(qn, doc.clone());
            }
        }
    }
    for symbol in symbols {
        if !labels::is_callable(&symbol.label) {
            continue;
        }
        if let Some(files) = only_files {
            if !files.contains(symbol.file_path.as_str()) {
                continue;
            }
        }
        let mut tokens: Vec<String> = Vec::new();
        tokens.extend(split_identifier(&symbol.name));
        tokens.extend(split_identifier(&symbol.qn));
        tokens.extend(split_identifier(&symbol.scope.join(".")));
        if let Some(names) = callees.get(&symbol.qn) {
            for name in names {
                tokens.extend(split_identifier(name));
            }
        }
        if let Some(doc) = docstrings.get(&symbol.qn) {
            tokens.extend(split_text(doc));
        }
        tokens.retain(|t| t.len() >= 2);
        tokens.sort();
        tokens.dedup();
        if tokens.is_empty() {
            continue;
        }
        if let Some(vector) = nomic::embed_tokens(tokens.iter().map(String::as_str)) {
            builder.add_vector(&symbol.qn, vector);
        }
    }
}

fn similarity_edges(builder: &mut GraphBuilder) {
    let by_qn: Vec<(String, Fingerprint)> = builder
        .nodes
        .iter()
        .filter(|(_, node)| labels::is_callable(&node.label))
        .filter_map(|(qn, node)| {
            let fp = node.properties.get("fp")?.as_str()?;
            Some((qn.clone(), Fingerprint::from_hex(fp)?))
        })
        .collect();
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for (idx, (_, fp)) in by_qn.iter().enumerate() {
        for band in 0..8 {
            let mut h = 0x9e37_79b9_7f4a_7c15u64;
            for j in 0..8 {
                h = h
                    .wrapping_mul(0x100_0000_01b3)
                    .wrapping_add(fp.hashes[band * 8 + j]);
            }
            buckets.entry(h).or_default().push(idx);
        }
    }
    let mut edges: HashMap<(usize, usize), f64> = HashMap::new();
    for bucket in buckets.values() {
        if bucket.len() > 64 || bucket.len() < 2 {
            continue;
        }
        for i in 0..bucket.len() {
            for j in i + 1..bucket.len() {
                let (a, b) = (bucket[i], bucket[j]);
                let score = by_qn[a].1.jaccard(&by_qn[b].1);
                if score >= 0.75 {
                    edges.insert((a.min(b), a.max(b)), score);
                }
            }
        }
    }
    let mut per_node: HashMap<usize, usize> = HashMap::new();
    for ((a, b), score) in edges {
        if per_node.get(&a).copied().unwrap_or(0) >= 3
            || per_node.get(&b).copied().unwrap_or(0) >= 3
        {
            continue;
        }
        *per_node.entry(a).or_insert(0) += 1;
        *per_node.entry(b).or_insert(0) += 1;
        let p = props(&[("score", &format!("{score:.3}")), ("via", "minhash")]);
        builder.add_edge(&by_qn[a].0, &by_qn[b].0, "SIMILAR_TO", p.clone());
        builder.add_edge(&by_qn[b].0, &by_qn[a].0, "SIMILAR_TO", p);
    }
}

fn semantic_related_edges(builder: &mut GraphBuilder, symbols: &[Symbol]) {
    let vectors: HashMap<String, Vec<i8>> = builder.vectors.clone();
    let mut tokens_by_qn: HashMap<String, Vec<String>> = HashMap::new();
    for symbol in symbols {
        if !labels::is_callable(&symbol.label) {
            continue;
        }
        let mut tokens = split_identifier(&symbol.name);
        tokens.extend(split_identifier(&symbol.qn));
        tokens.extend(split_identifier(&symbol.scope.join(".")));
        tokens.retain(|t| t.len() >= 3);
        tokens.sort();
        tokens.dedup();
        tokens_by_qn.insert(symbol.qn.clone(), tokens);
    }
    let mut df: HashMap<String, usize> = HashMap::new();
    for tokens in tokens_by_qn.values() {
        for token in tokens {
            *df.entry(token.clone()).or_insert(0) += 1;
        }
    }
    let rare_cutoff = (tokens_by_qn.len() / 100).max(3);
    for tokens in tokens_by_qn.values_mut() {
        tokens.retain(|t| df.get(t).copied().unwrap_or(0) <= rare_cutoff);
    }
    let mut postings: HashMap<String, Vec<String>> = HashMap::new();
    for (qn, tokens) in &tokens_by_qn {
        for token in tokens {
            postings.entry(token.clone()).or_default().push(qn.clone());
        }
    }
    let mut candidates: HashMap<(String, String), usize> = HashMap::new();
    for (qn, tokens) in &tokens_by_qn {
        let mut overlap: HashMap<&str, usize> = HashMap::new();
        for token in tokens {
            if let Some(qns) = postings.get(token) {
                if qns.len() > 2000 {
                    continue;
                }
                for other in qns {
                    if other != qn {
                        *overlap.entry(other.as_str()).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut ranked: Vec<(&str, usize)> = overlap.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (other, count) in ranked.into_iter().take(64) {
            if count >= 2 {
                let key = if qn.as_str() < other {
                    (qn.clone(), other.to_string())
                } else {
                    (other.to_string(), qn.clone())
                };
                candidates.insert(key, count);
            }
        }
    }
    let mut scored: Vec<(String, String, f64)> = Vec::new();
    for ((a, b), _) in candidates {
        let ta: HashSet<&String> = tokens_by_qn.get(&a).into_iter().flatten().collect();
        let tb: HashSet<&String> = tokens_by_qn.get(&b).into_iter().flatten().collect();
        if ta.is_empty() || tb.is_empty() {
            continue;
        }
        let shared = ta.intersection(&tb).count();
        if shared < 3 {
            continue;
        }
        let jaccard = shared as f64 / ta.union(&tb).count() as f64;
        if jaccard < 0.35 {
            continue;
        }
        let cosine = match (vectors.get(&a), vectors.get(&b)) {
            (Some(va), Some(vb)) => nomic::cosine_i8(va, vb) as f64,
            _ => 0.0,
        };
        let score = jaccard * 0.75 + cosine * 0.25;
        if score >= 0.65 {
            scored.push((a, b, score));
        }
    }
    scored.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
    });
    let mut per_node: HashMap<String, usize> = HashMap::new();
    for (a, b, score) in scored {
        if per_node.get(&a).copied().unwrap_or(0) >= 3
            || per_node.get(&b).copied().unwrap_or(0) >= 3
        {
            continue;
        }
        *per_node.entry(a.clone()).or_insert(0) += 1;
        *per_node.entry(b.clone()).or_insert(0) += 1;
        let p = props(&[("score", &format!("{score:.3}")), ("via", "semantic")]);
        builder.add_edge(&a, &b, "SEMANTICALLY_RELATED", p.clone());
        builder.add_edge(&b, &a, "SEMANTICALLY_RELATED", p);
    }
}

fn importance(builder: &mut GraphBuilder, symbols: &[Symbol]) {
    let mut weighted: HashMap<String, i64> = HashMap::new();
    for edge in builder.edges.values() {
        if matches!(
            edge.edge_type.as_str(),
            "CALLS" | "USAGE" | "CALL_REFERENCE" | "INHERITS" | "IMPLEMENTS" | "HTTP_CALLS"
        ) {
            *weighted.entry(edge.target_qn.clone()).or_insert(0) += 2;
            *weighted.entry(edge.source_qn.clone()).or_insert(0) += 1;
        }
    }
    for symbol in symbols {
        if let Some(score) = weighted.get(&symbol.qn) {
            builder.set_node_property(&symbol.qn, "importance", Value::Number((*score).into()));
        }
    }
}

fn config_links(builder: &mut GraphBuilder, parsed: &[ParsedFile], symbols: &[Symbol]) {
    let config_nodes: Vec<(String, String, String)> = builder
        .nodes
        .iter()
        .filter(|(_, n)| {
            n.label == labels::VARIABLE
                && n.properties
                    .get("config_key")
                    .and_then(Value::as_str)
                    .is_some()
        })
        .map(|(qn, n)| {
            (
                qn.clone(),
                n.properties
                    .get("config_key")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                n.file_path.clone(),
            )
        })
        .collect();
    if config_nodes.is_empty() {
        return;
    }
    let mut code_by_name: HashMap<String, Vec<&Symbol>> = HashMap::new();
    for symbol in symbols {
        if matches!(
            symbol.label.as_str(),
            "Function" | "Method" | "Class" | "Struct" | "Variable"
        ) {
            code_by_name
                .entry(normalize_config_key(&symbol.name))
                .or_default()
                .push(symbol);
        }
    }
    for (config_qn, key, config_file) in config_nodes {
        let normalized = normalize_config_key(&key);
        if normalized.len() < 3 {
            continue;
        }
        let mut matches: Vec<(f64, &Symbol)> = Vec::new();
        for (name, list) in &code_by_name {
            let confidence = if *name == normalized {
                0.85
            } else if name.contains(&normalized) || normalized.contains(name) {
                0.75
            } else {
                continue;
            };
            for symbol in list {
                if same_component(&config_file, &symbol.file_path) {
                    matches.push((confidence, symbol));
                }
            }
        }
        matches.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.qn.cmp(&b.1.qn))
        });
        for (confidence, symbol) in matches.into_iter().take(8) {
            builder.add_edge(
                &symbol.qn,
                &config_qn,
                "CONFIGURES",
                props(&[
                    ("strategy", "key_symbol"),
                    ("confidence", &format!("{confidence:.2}")),
                    ("config_key", &key),
                ]),
            );
        }
    }
    let _ = parsed;
}

fn same_component(config_file: &str, code_file: &str) -> bool {
    let c = config_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let k = code_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    if c.is_empty() || k.is_empty() {
        return true;
    }
    c == k
        || c.starts_with(&format!("{k}/"))
        || k.starts_with(&format!("{c}/"))
        || c.split('/').next() == k.split('/').next()
}

fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}
