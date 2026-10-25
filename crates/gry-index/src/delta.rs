//! Incremental delta indexing.
//!
//! A one-file edit repairs only the files whose *resolution inputs* changed:
//! the edited/deleted files, files that reference a changed symbol name, and
//! files with edges into the changed files. Everything else keeps its already
//! published nodes and edges. The graph is still resolved with the complete
//! symbol registry, so cross-file relationships remain correct.

use crate::build::{build, BuildOptions, ParsedFile, PartialInput};
use crate::discover::{DiscoveredFile, Discovery, IndexMode};
use crate::graph::qn_symbol;
use crate::pipeline::{
    encode_cache, excluded_report, parse_partial_rows, CoverageSummary, IndexReport, IndexRequest,
    SkippedFile, EXTRACTOR_VERSION,
};
use crate::resolve::{RegistrySeed, Symbol};
use gry_core::config::Config;
use gry_core::fqn::{fqn_compute, fqn_module_dir, FILE_QN_NAME};
use gry_core::types::{FileHash, NewNode};
use gry_parse::extract_file_with_previous;
use gry_parse::model::FileExtract;
use gry_store::{
    CoverageMeta, CoverageRow, FileMetaRow, ProjectRecord, ProjectWrite, RefRow, Store, SymbolRow,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

const MAX_DELTA_FILES: usize = 64;

pub struct DeltaContext<'a> {
    pub config: &'a Config,
    pub store: &'a mut Store,
    pub project: &'a str,
    pub root: &'a Path,
    pub mode: IndexMode,
    pub discovery: &'a Discovery,
    pub changed: &'a [&'a DiscoveredFile],
    pub deleted: &'a [String],
    pub known_files: &'a HashSet<String>,
    pub existing: &'a ProjectRecord,
    pub request: &'a IndexRequest,
    pub started: Instant,
}

pub fn try_incremental(ctx: DeltaContext<'_>) -> gry_core::GryResult<Option<IndexReport>> {
    if !ctx.store.supports_delta() {
        return Ok(None);
    }
    let changed_paths: Vec<String> = ctx.changed.iter().map(|f| f.rel_path.clone()).collect();
    if changed_paths.len() + ctx.deleted.len() > MAX_DELTA_FILES {
        return Ok(None);
    }

    let t_parse = Instant::now();
    // Previous extraction enables reusing per-symbol bodies that did not change.
    let mut previous: HashMap<String, FileExtract> = HashMap::new();
    for file in ctx.changed {
        if let Ok(Some((sha, data))) = ctx.store.parse_cache_entry(ctx.project, &file.rel_path) {
            if sha == file.sha256 {
                continue;
            }
            if let Some(extract) = decode_cache(&data) {
                previous.insert(file.rel_path.clone(), extract);
            }
        }
    }
    // 1. Parse changed files.
    let parsed_changed: Vec<(String, Result<FileExtract, String>)> = ctx
        .changed
        .par_iter()
        .map(|file| match std::fs::read(&file.abs_path) {
            Ok(bytes) => {
                let lang = gry_core::language::detect(&file.rel_path)
                    .unwrap_or_else(|| gry_core::language::detect("x.txt").unwrap());
                let prev = previous.get(&file.rel_path);
                (
                    file.rel_path.clone(),
                    Ok(extract_file_with_previous(
                        &file.rel_path,
                        lang,
                        &bytes,
                        prev,
                    )),
                )
            }
            Err(e) => (file.rel_path.clone(), Err(e.to_string())),
        })
        .collect();
    let mut new_extracts: HashMap<String, FileExtract> = HashMap::new();
    let mut parse_errors: Vec<(String, String)> = Vec::new();
    for (rel, result) in parsed_changed {
        match result {
            Ok(extract) => {
                new_extracts.insert(rel, extract);
            }
            Err(e) => parse_errors.push((rel, e)),
        }
    }

    if crate::pipeline::timing_enabled() {
        eprintln!(
            "[gry] delta parse changed           {:>7} ms",
            t_parse.elapsed().as_millis()
        );
    }
    let t_names = Instant::now();
    // A body-only change that leaves calls/usages/imports and line ranges
    // identical does not invalidate any edge. Nodes still need an update
    // (fingerprints, complexity), but no edge is deleted or re-resolved.
    let mut nodes_only = ctx.deleted.is_empty()
        && ctx
            .changed
            .iter()
            .all(|f| ctx.known_files.contains(&f.rel_path))
        && ctx.changed.iter().all(|f| {
            previous
                .get(&f.rel_path)
                .zip(new_extracts.get(&f.rel_path))
                .map(|(old, new)| structural_signature(old) == structural_signature(new))
                .unwrap_or(false)
        });

    let affected_input = {
        let mut set: BTreeSet<String> = changed_paths.iter().cloned().collect();
        set.extend(ctx.deleted.iter().cloned());
        set
    };
    let structure_changed = !ctx.deleted.is_empty()
        || ctx
            .changed
            .iter()
            .any(|f| !ctx.known_files.contains(&f.rel_path));
    let mut changed_names: Vec<String> = Vec::new();
    let mut affected: BTreeSet<String>;
    if nodes_only {
        affected = changed_paths.iter().cloned().collect();
    } else {
        let old_affected_symbols = ctx.store.symbols_for_files(
            ctx.project,
            &affected_input.iter().cloned().collect::<Vec<_>>(),
        )?;
        let new_affected_symbols =
            symbols_from_extracts(ctx.project, ctx.discovery, &new_extracts, &changed_paths);
        changed_names = changed_symbol_names(&old_affected_symbols, &new_affected_symbols);

        if crate::pipeline::timing_enabled() {
            eprintln!(
                "[gry] delta name diff               {:>7} ms",
                t_names.elapsed().as_millis()
            );
        }
        let t_refs = Instant::now();
        // 3. Module/file hints for added and deleted files.
        let mut hints: BTreeSet<String> = BTreeSet::new();
        for rel in changed_paths.iter().chain(ctx.deleted.iter()) {
            let added = !ctx.known_files.contains(rel);
            if added || ctx.deleted.contains(rel) {
                hints.insert(rel.clone());
                hints.insert(stem_of(rel));
                if let Some(base) = rel.rsplit('/').next() {
                    hints.insert(base.to_string());
                    hints.insert(stem_of(base));
                }
            }
        }
        hints.extend(changed_names.iter().cloned());

        affected = affected_input.clone();
        if !changed_names.is_empty() {
            for file in ctx.store.files_for_ref_names(ctx.project, &changed_names)? {
                affected.insert(file);
            }
        }
        // Callers only need re-resolution when the candidate set or module map
        // changed. A pure body edit keeps every caller edge valid.
        if !changed_names.is_empty() || structure_changed {
            for file in ctx.store.dependent_files(
                ctx.project,
                &affected_input.iter().cloned().collect::<Vec<_>>(),
            )? {
                affected.insert(file);
            }
        }
        if !hints.is_empty() {
            for (file, spec) in ctx.store.import_refs(ctx.project)? {
                if hints.iter().any(|h| import_matches(h, &spec)) {
                    affected.insert(file);
                }
            }
        }
        if !hints.is_empty() {
            let lookup: Vec<String> = hints.iter().cloned().collect();
            if let Ok(files) = ctx.store.files_for_ref_names(ctx.project, &lookup) {
                affected.extend(files);
            }
        }
        for deleted in ctx.deleted {
            affected.remove(deleted);
        }
        // A module node can be shared by several files (C header + source,
        // Python package __init__, ...). Rebuilding one file must also rebuild
        // its module siblings, otherwise their module-level edges are lost.
        let module_to_files: HashMap<String, Vec<String>> = {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            for file in &ctx.discovery.files {
                let module = fqn_module_dir(ctx.project, &file.rel_path, file.module_is_dir);
                map.entry(module).or_default().push(file.rel_path.clone());
            }
            map
        };
        let mut siblings: BTreeSet<String> = BTreeSet::new();
        for file in affected.clone() {
            let Some(discovered) = ctx.discovery.files.iter().find(|f| f.rel_path == file) else {
                continue;
            };
            let module = fqn_module_dir(ctx.project, &file, discovered.module_is_dir);
            if let Some(files) = module_to_files.get(&module) {
                for sibling in files {
                    if !ctx.deleted.contains(sibling) {
                        siblings.insert(sibling.clone());
                    }
                }
            }
        }
        affected.extend(siblings);
        if crate::pipeline::timing_enabled() {
            eprintln!(
                "[gry] delta affected/refs           {:>7} ms",
                t_refs.elapsed().as_millis()
            );
        }
    }
    let cap = (ctx.discovery.files.len() / 3).max(MAX_DELTA_FILES);
    if affected.len() > cap {
        return Ok(None);
    }
    if crate::pipeline::timing_enabled() {
        eprintln!(
            "[gry] delta: changed={} deleted={} names={} affected={}",
            changed_paths.len(),
            ctx.deleted.len(),
            changed_names.len(),
            affected.len()
        );
    }

    // 4. Load extraction for every affected file.
    let by_path: HashMap<&str, &DiscoveredFile> = ctx
        .discovery
        .files
        .iter()
        .map(|f| (f.rel_path.as_str(), f))
        .collect();
    let mut parsed: Vec<ParsedFile> = Vec::with_capacity(affected.len());
    for rel in &affected {
        let Some(discovered) = by_path.get(rel.as_str()) else {
            continue;
        };
        let extract = if let Some(extract) = new_extracts.get(rel) {
            extract.clone()
        } else {
            match ctx.store.parse_cache_entry(ctx.project, rel)? {
                Some((sha, data)) if sha == discovered.sha256 => match decode_cache(&data) {
                    Some(extract) => extract,
                    None => {
                        if crate::pipeline::timing_enabled() {
                            eprintln!("[gry] delta fallback: decode failed for {rel}");
                        }
                        return Ok(None);
                    }
                },
                other => {
                    if crate::pipeline::timing_enabled() {
                        eprintln!(
                            "[gry] delta fallback: cache missing/stale for {rel} (entry={}, sha={})",
                            other.is_some(),
                            discovered.sha256
                        );
                    }
                    return Ok(None);
                }
            }
        };
        parsed.push(ParsedFile {
            discovered: (*discovered).clone(),
            extract,
        });
    }
    parsed.sort_by(|a, b| a.discovered.rel_path.cmp(&b.discovered.rel_path));

    if crate::pipeline::timing_enabled() {
        eprintln!(
            "[gry] delta extracts                {:>7} ms",
            ctx.started.elapsed().as_millis()
        );
    }
    // 5. Complete registry seed from the published graph. A nodes-only update
    // never resolves edges, so it does not need the registry at all.
    let (external_symbols, seed, file_qn, module_qn, external_qns, extra_nodes) = if nodes_only {
        (
            Vec::new(),
            RegistrySeed::default(),
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            Vec::new(),
        )
    } else {
        let t = Instant::now();
        let all_symbols = ctx.store.symbols(ctx.project)?;
        let affected_set: HashSet<&str> = affected.iter().map(String::as_str).collect();
        let deleted_set: HashSet<&str> = ctx.deleted.iter().map(String::as_str).collect();
        let external_symbols: Vec<Symbol> = all_symbols
            .iter()
            .filter(|s| !affected_set.contains(s.file_path.as_str()))
            .filter(|s| !deleted_set.contains(s.file_path.as_str()))
            .map(symbol_from_row)
            .collect();
        let t_seed = Instant::now();
        let seed = build_seed(ctx.project, ctx.discovery, ctx.store)?;
        let (file_qn, module_qn) = file_maps(ctx.project, ctx.discovery);
        let t_qns = Instant::now();
        let mut external_qns = ctx.store.all_qns(ctx.project)?;
        external_qns.extend(file_qn.values().cloned());
        external_qns.extend(module_qn.values().cloned());
        external_qns.sort();
        external_qns.dedup();
        let t_nodes = Instant::now();
        let extra_nodes = config_nodes(ctx.store, ctx.project)?;
        if crate::pipeline::timing_enabled() {
            eprintln!(
                "[gry] delta symbols/seed/qns/nodes  {:>7} / {:>7} / {:>7} / {:>7} ms ({} symbols)",
                t.elapsed().as_millis(),
                t_seed.elapsed().as_millis(),
                t_qns.elapsed().as_millis(),
                t_nodes.elapsed().as_millis(),
                all_symbols.len()
            );
        }
        (
            external_symbols,
            seed,
            file_qn,
            module_qn,
            external_qns,
            extra_nodes,
        )
    };

    let branch = git_line(ctx.root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|s| !s.is_empty() && s != "HEAD");
    let working_tree = git_line(ctx.root, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if nodes_only {
        // Branch/working-tree visibility is part of the graph; a checkout is
        // a structural update even when source files did not change.
        let mut old_branch = false;
        for node in ctx.store.nodes_with_label(ctx.project, "Branch")? {
            let name = node
                .property_str("branch")
                .unwrap_or(&node.name)
                .to_string();
            let dirty = node.property_bool("dirty").unwrap_or(false);
            if name == "working-tree" {
                if dirty != working_tree {
                    old_branch = true;
                }
            } else if Some(&name) != branch.as_ref() {
                old_branch = true;
            }
        }
        if old_branch {
            nodes_only = false;
        }
    }
    let affected_set: HashSet<&str> = affected.iter().map(String::as_str).collect();
    let options = BuildOptions {
        project: ctx.project.to_string(),
        root_path: ctx.root.to_string_lossy().to_string(),
        mode: ctx.mode.as_str().to_string(),
        semantic_edges: false,
        branch,
        working_tree,
        nodes_only,
        partial: Some(PartialInput {
            external_symbols,
            seed,
            file_qn,
            module_qn,
            external_qns,
            extra_nodes,
        }),
    };
    let phase_start = Instant::now();
    let result = build(&parsed, &options);
    if crate::pipeline::timing_enabled() {
        eprintln!(
            "[gry] delta build                   {:>7} ms (extract calls={} usages={} defs={})",
            phase_start.elapsed().as_millis(),
            parsed.iter().map(|p| p.extract.calls.len()).sum::<usize>(),
            parsed.iter().map(|p| p.extract.usages.len()).sum::<usize>(),
            parsed.iter().map(|p| p.extract.defs.len()).sum::<usize>(),
        );
        use std::collections::BTreeMap as B;
        let mut counts: B<&str, usize> = B::new();
        for edge in result.builder.edges.values() {
            *counts.entry(edge.edge_type.as_str()).or_default() += 1;
        }
        eprintln!("[gry] delta edges: {counts:?}");
    }
    let affected_symbols: Vec<SymbolRow> = if nodes_only {
        Vec::new()
    } else {
        result
            .symbols
            .iter()
            .filter(|s| affected_set.contains(s.file_path.as_str()))
            .map(symbol_to_row)
            .collect()
    };
    let (nodes, edges, vectors) = result.builder.finish();

    // 6. Remove QNs that disappeared from the affected files.
    let mut old_files: Vec<String> = affected.iter().cloned().collect();
    old_files.extend(ctx.deleted.iter().cloned());
    let old_qns = ctx.store.qns_for_files(ctx.project, &old_files)?;
    let old_qn_count = old_qns.len();
    let new_qns: HashSet<&str> = nodes.iter().map(|n| n.qualified_name.as_str()).collect();
    let removed_qns: Vec<String> = old_qns
        .into_iter()
        .filter(|qn| !new_qns.contains(qn.as_str()))
        .collect();
    if crate::pipeline::timing_enabled() {
        eprintln!(
            "[gry] delta nodes: old={} new={} removed={}",
            old_qn_count,
            new_qns.len(),
            removed_qns.len()
        );
    }

    let refs = if nodes_only {
        Vec::new()
    } else {
        refs_from_extracts(&parsed)
    };

    // 7. Coverage and hashes.
    let mut coverage: Vec<CoverageRow> = ctx
        .discovery
        .coverage
        .iter()
        .map(|c| CoverageRow {
            rel_path: c.rel_path.clone(),
            kind: c.kind.clone(),
            detail: c.detail.clone(),
        })
        .collect();
    for (rel, err) in &parse_errors {
        coverage.push(CoverageRow {
            rel_path: rel.clone(),
            kind: "read".to_string(),
            detail: err.clone(),
        });
    }
    let mut parse_partial_count = 0usize;
    for file in &parsed {
        if file.extract.parse_gaps.is_empty() {
            continue;
        }
        let ranges: Vec<String> = file
            .extract
            .parse_gaps
            .iter()
            .map(|g| format!("{}-{}", g.start_line, g.end_line))
            .collect();
        let unusable = file
            .extract
            .parse_gaps
            .iter()
            .any(|g| g.reason == "parse_unusable");
        coverage.push(CoverageRow {
            rel_path: file.discovered.rel_path.clone(),
            kind: if unusable {
                "parse_unusable".to_string()
            } else {
                "parse_partial".to_string()
            },
            detail: ranges.join(","),
        });
        parse_partial_count += 1;
    }

    let file_hashes: Vec<FileHash> = ctx
        .discovery
        .files
        .iter()
        .map(|f| FileHash {
            rel_path: f.rel_path.clone(),
            sha256: f.sha256.clone(),
            mtime_ns: f.mtime_ns,
            size: f.size,
        })
        .collect();
    let parse_cache: Vec<(String, String, Vec<u8>)> = parsed
        .iter()
        .filter(|f| affected_set.contains(f.discovered.rel_path.as_str()))
        .filter_map(|file| {
            let data = encode_cache(&file.extract)?;
            Some((
                file.discovered.rel_path.clone(),
                file.discovered.sha256.clone(),
                data,
            ))
        })
        .collect();
    let file_meta: Vec<FileMetaRow> = if nodes_only {
        Vec::new()
    } else {
        ctx.discovery
            .files
            .iter()
            .filter(|f| affected_set.contains(f.rel_path.as_str()))
            .map(|f| FileMetaRow {
                rel_path: f.rel_path.clone(),
                language: f.language.clone(),
                module_is_dir: f.module_is_dir,
            })
            .collect()
    };
    let root_string = ctx.root.to_string_lossy().to_string();
    let generation = gry_store::new_generation();
    let coverage_meta = CoverageMeta {
        generation: generation.clone(),
        index_mode: ctx.mode.as_str().to_string(),
        recorded_at: gry_store::now_iso(),
        recording_status: "complete".to_string(),
        ignored_files_stored: ctx.discovery.ignored_files.len() as i64,
        ignored_files_total: ctx.discovery.ignored_files.len() as i64,
        coverage_version: 1,
        hash_records_complete: true,
    };
    let vectors: Vec<(String, Vec<i8>)> = vectors.into_iter().collect();
    let affected_vec: Vec<String> = affected.iter().cloned().collect();
    let mut write = ProjectWrite::new(ctx.project, &root_string, ctx.mode.as_str(), &generation);
    write.files = &file_hashes;
    write.nodes = &nodes;
    write.edges = &edges;
    write.vectors = &vectors;
    write.parse_cache = &parse_cache;
    write.coverage = &coverage;
    write.coverage_meta = Some(&coverage_meta);
    write.symbols = &affected_symbols;
    write.refs = &refs;
    write.file_meta = &file_meta;
    write.affected_files = &affected_vec;
    write.deleted_files = ctx.deleted;
    write.removed_qns = &removed_qns;
    write.nodes_only = nodes_only;
    write.gc_globals = !nodes_only
        && (!removed_qns.is_empty()
            || !ctx.deleted.is_empty()
            || parsed.iter().any(|f| {
                !f.extract.env_accesses.is_empty()
                    || !f.extract.channels.is_empty()
                    || !f.extract.spawns.is_empty()
            }));
    write.replace_nodes = false;
    if nodes_only {
        write.edges = &[];
    }
    let phase_start = Instant::now();
    let stats = ctx.store.publish(&write)?;
    if crate::pipeline::timing_enabled() {
        eprintln!(
            "[gry] delta publish                 {:>7} ms",
            phase_start.elapsed().as_millis()
        );
    }
    let (total_nodes, total_edges) = ctx.store.counts(ctx.project)?;
    ctx.store.set_meta("extractor_version", EXTRACTOR_VERSION)?;
    ctx.store.checkpoint_passive()?;

    let mut artifact = None;
    let mut artifact_error = None;
    if ctx.request.persistence {
        let path = Config::artifact_path(ctx.root);
        match ctx.store.export_artifact(ctx.project, &path) {
            Ok(_) => artifact = Some(path.to_string_lossy().to_string()),
            Err(e) => artifact_error = Some(e.to_string()),
        }
    }

    let parsed_files = new_extracts.len();
    let skipped: Vec<SkippedFile> = ctx
        .discovery
        .coverage
        .iter()
        .filter(|c| c.kind != "parse_partial" && c.kind != "parse_unusable")
        .map(|c| SkippedFile {
            path: c.rel_path.clone(),
            reason: c.detail.clone(),
            phase: c.kind.clone(),
        })
        .chain(parse_errors.iter().map(|(rel, err)| SkippedFile {
            path: rel.clone(),
            reason: err.clone(),
            phase: "read".to_string(),
        }))
        .collect();
    let report = IndexReport {
        project: ctx.project.to_string(),
        root_path: root_string,
        mode: ctx.mode.as_str().to_string(),
        generation,
        files: ctx.discovery.files.len(),
        source_bytes: ctx.discovery.source_bytes,
        nodes: total_nodes,
        edges: total_edges,
        parsed_files,
        reused_files: ctx.discovery.files.len().saturating_sub(parsed_files),
        deleted_files: ctx.deleted.len(),
        duration_ms: ctx.started.elapsed().as_millis(),
        unchanged: false,
        skipped,
        excluded: excluded_report(ctx.discovery),
        parse_partial: parse_partial_rows(&coverage),
        coverage: CoverageSummary {
            gaps: coverage.len(),
            parse_partial: parse_partial_count,
            ignored_files: ctx.discovery.ignored_files.len(),
            ignored_total: ctx.discovery.ignored_files.len(),
            hash_records_complete: true,
        },
        artifact,
        artifact_error,
        cross_repo: None,
    };
    let _ = ctx
        .store
        .set_meta("last_index_report", &serde_json::to_string(&report)?);
    let _ = (ctx.existing, stats);
    Ok(Some(report))
}

/// Signature of everything that can change an edge endpoint or its line.
/// Fields are hashed directly to avoid building a large intermediate string.
fn structural_signature(extract: &FileExtract) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |part: &str| {
        h = h
            .wrapping_mul(0x0000_0100_0000_01b3)
            .wrapping_add(gry_core::tokenize::stable_hash(part));
    };
    for def in &extract.defs {
        mix("d");
        mix(&def.name);
        mix(&def.label);
        for part in &def.scope {
            mix(part);
        }
        mix(&def.start_line.to_string());
        mix(&def.end_line.to_string());
    }
    for call in &extract.calls {
        mix("c");
        mix(&call.name);
        mix(call.receiver.as_deref().unwrap_or(""));
        mix(call.receiver_type.as_deref().unwrap_or(""));
        for part in &call.scope {
            mix(part);
        }
        mix(&call.line.to_string());
        mix(&call.kind);
    }
    for usage in &extract.usages {
        mix("u");
        mix(&usage.name);
        for part in &usage.scope {
            mix(part);
        }
        mix(&usage.line.to_string());
        mix(&usage.kind);
    }
    for import in &extract.imports {
        mix("i");
        mix(&import.specifier);
        mix(import.local_name.as_deref().unwrap_or(""));
        mix(&import.line.to_string());
        mix(&import.kind);
    }
    for route in &extract.routes {
        mix("r");
        mix(&route.method);
        mix(&route.path);
        mix(route.handler.as_deref().unwrap_or(""));
        mix(&route.line.to_string());
    }
    for env in &extract.env_accesses {
        mix("e");
        mix(&env.key);
        for part in &env.scope {
            mix(part);
        }
        mix(&env.line.to_string());
    }
    for channel in &extract.channels {
        mix("h");
        mix(&channel.name);
        mix(&channel.action);
        for part in &channel.scope {
            mix(part);
        }
        mix(&channel.line.to_string());
    }
    for spawn in &extract.spawns {
        mix("s");
        mix(&spawn.program);
        for part in &spawn.scope {
            mix(part);
        }
        mix(&spawn.line.to_string());
    }
    for http in &extract.http_calls {
        mix("t");
        mix(&http.method);
        mix(&http.url_path);
        for part in &http.scope {
            mix(part);
        }
        mix(&http.line.to_string());
    }
    for reference in &extract.file_refs {
        mix("f");
        mix(&reference.path);
        mix(&reference.line.to_string());
    }
    for reference in &extract.doc_refs {
        mix("o");
        mix(&reference.raw);
        mix(&reference.target_kind);
        mix(&reference.line.to_string());
    }
    for section in &extract.sections {
        mix("n");
        mix(&section.title);
        mix(&section.level.to_string());
        mix(&section.start_line.to_string());
        mix(&section.end_line.to_string());
    }
    for key in &extract.config_keys {
        mix("k");
        mix(&key.key);
        mix(&key.format);
        mix(&key.line.to_string());
    }
    for node in &extract.infra_nodes {
        mix("g");
        mix(&node.qualified_name);
        mix(&node.label);
        mix(&node.start_line.to_string());
    }
    for edge in &extract.infra_edges {
        mix("j");
        mix(&edge.source_qn);
        mix(&edge.target_qn);
        mix(&edge.edge_type);
    }
    if let Some(package) = &extract.package {
        mix("p");
        mix(&package.name);
        mix(&package.ecosystem);
        for dep in &package.dependencies {
            mix(dep);
        }
    }
    if let Some(adr) = &extract.adr {
        mix("a");
        mix(&adr.id);
        mix(&adr.title);
    }
    for op in &extract.openapi {
        mix("q");
        mix(&op.method);
        mix(&op.path);
        mix(&op.line.to_string());
    }
    for gap in &extract.parse_gaps {
        mix("x");
        mix(&gap.start_line.to_string());
        mix(&gap.end_line.to_string());
        mix(&gap.reason);
    }
    format!("{h:016x}")
}

fn decode_cache(data: &[u8]) -> Option<FileExtract> {
    crate::pipeline::decode_cache(data)
}

fn stem_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(base)
        .to_string()
}

fn import_matches(hint: &str, specifier: &str) -> bool {
    if hint.is_empty() || specifier.is_empty() {
        return false;
    }
    let spec = specifier.replace([':', '.', '\\', '@', '/'], "/");
    let hint = hint.replace([':', '.', '\\', '@', '/'], "/");
    spec == hint
        || spec.ends_with(&format!("/{hint}"))
        || hint.ends_with(&format!("/{spec}"))
        || spec.contains(&hint)
}

fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn symbols_from_extracts(
    project: &str,
    discovery: &Discovery,
    extracts: &HashMap<String, FileExtract>,
    changed_paths: &[String],
) -> Vec<SymbolRow> {
    let by_path: HashMap<&str, &DiscoveredFile> = discovery
        .files
        .iter()
        .map(|f| (f.rel_path.as_str(), f))
        .collect();
    let mut out = Vec::new();
    for rel in changed_paths {
        let Some(extract) = extracts.get(rel) else {
            continue;
        };
        let Some(file) = by_path.get(rel.as_str()) else {
            continue;
        };
        let module_qn = fqn_module_dir(project, rel, file.module_is_dir);
        for def in &extract.defs {
            out.push(SymbolRow {
                qn: qn_symbol(project, rel, &def.scope, &def.name),
                name: def.name.clone(),
                label: def.label.clone(),
                file_path: rel.clone(),
                language: file.language.clone(),
                module_qn: module_qn.clone(),
                scope: def.scope.clone(),
                start_line: def.start_line,
                is_test: def.is_test,
            });
        }
    }
    out
}

fn changed_symbol_names(old: &[SymbolRow], new: &[SymbolRow]) -> Vec<String> {
    let mut old_map: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut new_map: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for symbol in old {
        old_map.entry(symbol.name.clone()).or_default().insert((
            symbol.qn.clone(),
            format!("{}:{}", symbol.label, symbol.is_test),
        ));
    }
    for symbol in new {
        new_map.entry(symbol.name.clone()).or_default().insert((
            symbol.qn.clone(),
            format!("{}:{}", symbol.label, symbol.is_test),
        ));
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    for (name, qns) in &old_map {
        if new_map.get(name) != Some(qns) {
            names.insert(name.clone());
        }
    }
    for (name, qns) in &new_map {
        if old_map.get(name) != Some(qns) {
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

fn symbol_from_row(row: &SymbolRow) -> Symbol {
    Symbol {
        qn: row.qn.clone(),
        name: row.name.clone(),
        label: row.label.clone(),
        file_path: row.file_path.clone(),
        language: row.language.clone(),
        module_qn: row.module_qn.clone(),
        scope: row.scope.clone(),
        start_line: row.start_line,
        is_test: row.is_test,
    }
}

pub(crate) fn symbol_to_row(symbol: &Symbol) -> SymbolRow {
    SymbolRow {
        qn: symbol.qn.clone(),
        name: symbol.name.clone(),
        label: symbol.label.clone(),
        file_path: symbol.file_path.clone(),
        language: symbol.language.clone(),
        module_qn: symbol.module_qn.clone(),
        scope: symbol.scope.clone(),
        start_line: symbol.start_line,
        is_test: symbol.is_test,
    }
}

fn file_maps(
    project: &str,
    discovery: &Discovery,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut files = HashMap::new();
    let mut modules = HashMap::new();
    for file in &discovery.files {
        files.insert(
            file.rel_path.clone(),
            fqn_compute(project, &file.rel_path, Some(FILE_QN_NAME)),
        );
        modules.insert(
            file.rel_path.clone(),
            fqn_module_dir(project, &file.rel_path, file.module_is_dir),
        );
    }
    (files, modules)
}

fn build_seed(
    project: &str,
    discovery: &Discovery,
    store: &Store,
) -> gry_core::GryResult<RegistrySeed> {
    let mut seed = RegistrySeed::default();
    for file in &discovery.files {
        let module_qn = fqn_module_dir(project, &file.rel_path, file.module_is_dir);
        let stem = file
            .rel_path
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| file.rel_path.clone());
        seed.module_by_stem
            .entry(stem.clone())
            .or_insert_with(|| module_qn.clone());
        if stem.ends_with("/__init__") || stem.ends_with("/index") {
            if let Some((dir, _)) = stem.rsplit_once('/') {
                seed.module_by_stem
                    .entry(dir.to_string())
                    .or_insert_with(|| module_qn.clone());
            }
        }
    }
    for node in store.nodes_with_label(project, "Package")? {
        if node.property_bool("external").unwrap_or(false) {
            continue;
        }
        let dir = node
            .file_path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        let ecosystem = node.property_str("ecosystem").unwrap_or("").to_string();
        seed.package_roots
            .entry(node.name.clone())
            .or_insert_with(|| dir.clone());
        seed.package_names
            .entry(node.name.clone())
            .or_insert(ecosystem);
    }
    for node in store.nodes_with_label(project, "ADR")? {
        if let Some(id) = node.property_str("adr_id") {
            seed.adr_by_id
                .entry(id.to_ascii_uppercase())
                .or_insert_with(|| node.qualified_name.clone());
        }
    }
    Ok(seed)
}

fn config_nodes(store: &Store, project: &str) -> gry_core::GryResult<Vec<NewNode>> {
    let mut out = Vec::new();
    for node in store.nodes_with_label(project, "Variable")? {
        if node.property_str("config_key").is_none() {
            continue;
        }
        out.push(NewNode {
            label: node.label,
            name: node.name,
            qualified_name: node.qualified_name,
            file_path: node.file_path,
            start_line: node.start_line,
            end_line: node.end_line,
            properties: node.properties,
        });
    }
    Ok(out)
}

pub(crate) fn refs_from_extracts(parsed: &[ParsedFile]) -> Vec<RefRow> {
    let mut set: BTreeSet<(String, String, String)> = BTreeSet::new();
    for file in parsed {
        let rel = file.discovered.rel_path.clone();
        for call in &file.extract.calls {
            set.insert((rel.clone(), call.name.clone(), "call".to_string()));
        }
        for usage in &file.extract.usages {
            set.insert((rel.clone(), usage.name.clone(), "usage".to_string()));
        }
        for import in &file.extract.imports {
            set.insert((rel.clone(), import.specifier.clone(), "import".to_string()));
        }
        for def in &file.extract.defs {
            for (base, implements) in &def.bases {
                set.insert((
                    rel.clone(),
                    base.clone(),
                    if *implements { "implements" } else { "base" }.to_string(),
                ));
            }
            for decorator in &def.decorators {
                set.insert((rel.clone(), decorator.clone(), "decorator".to_string()));
            }
        }
        for reference in &file.extract.doc_refs {
            set.insert((rel.clone(), reference.raw.clone(), "doc_ref".to_string()));
        }
        for reference in &file.extract.file_refs {
            set.insert((rel.clone(), reference.path.clone(), "file_ref".to_string()));
        }
        for route in &file.extract.routes {
            if let Some(handler) = &route.handler {
                set.insert((rel.clone(), handler.clone(), "handler".to_string()));
            }
        }
    }
    set.into_iter()
        .map(|(file_path, name, kind)| RefRow {
            file_path,
            name,
            kind,
        })
        .collect()
}
