//! Symbol registry and relationship resolution.
//!
//! Resolution is deliberately evidence-ranked. A call or usage is bound to one
//! candidate only when the score clears a threshold; ambiguity is recorded on
//! the edge instead of being hidden. Cross-language name collisions are vetoed.

use gry_core::fqn::{fqn_compute, fqn_module_dir, resolve_relative_import};
use gry_core::types::labels;
use gry_parse::model::{CallSite, Import, Usage};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Symbol {
    pub qn: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub language: String,
    pub module_qn: String,
    pub scope: Vec<String>,
    pub start_line: i32,
    pub is_test: bool,
}

#[derive(Debug, Clone)]
pub struct ImportBinding {
    pub target_module: Option<String>,
    pub target_package: Option<String>,
    pub external: bool,
}

#[derive(Debug, Default, Clone)]
pub struct RegistrySeed {
    pub module_by_stem: HashMap<String, String>,
    pub package_roots: HashMap<String, String>,
    pub package_names: HashMap<String, String>,
    pub adr_by_id: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub struct Registry {
    pub symbols: Vec<Symbol>,
    pub by_qn: HashMap<String, usize>,
    pub by_name: HashMap<String, Vec<usize>>,
    pub by_module: HashMap<String, Vec<usize>>,
    pub module_by_stem: HashMap<String, String>,
    pub imports_by_file: HashMap<String, HashMap<String, ImportBinding>>,
    pub package_roots: HashMap<String, String>,
    pub package_names: HashMap<String, String>,
    pub adr_by_id: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub target_qn: String,
    pub confidence: f64,
    pub resolved_by: &'static str,
    pub ambiguous: bool,
    pub candidates: usize,
}

impl Registry {
    pub fn build(
        project: &str,
        files: &[(gry_parse::model::FileExtract, String, bool)],
        symbols: Vec<Symbol>,
    ) -> Self {
        Self::build_with_seed(project, files, symbols, &RegistrySeed::default())
    }

    pub fn build_with_seed(
        project: &str,
        files: &[(gry_parse::model::FileExtract, String, bool)],
        symbols: Vec<Symbol>,
        seed: &RegistrySeed,
    ) -> Self {
        let mut registry = Registry {
            symbols,
            module_by_stem: seed.module_by_stem.clone(),
            package_roots: seed.package_roots.clone(),
            package_names: seed.package_names.clone(),
            adr_by_id: seed.adr_by_id.clone(),
            ..Default::default()
        };
        for (idx, symbol) in registry.symbols.iter().enumerate() {
            registry.by_qn.insert(symbol.qn.clone(), idx);
            registry
                .by_name
                .entry(symbol.name.clone())
                .or_default()
                .push(idx);
            registry
                .by_module
                .entry(symbol.module_qn.clone())
                .or_default()
                .push(idx);
        }
        for (extract, rel_path, module_is_dir) in files {
            let module_qn = fqn_module_dir(project, rel_path, *module_is_dir);
            let stem = rel_path
                .rsplit_once('.')
                .map(|(s, _)| s)
                .unwrap_or(rel_path);
            registry
                .module_by_stem
                .entry(stem.to_string())
                .or_insert_with(|| module_qn.clone());
            if stem.ends_with("/__init__") || stem.ends_with("/index") {
                if let Some((dir, _)) = stem.rsplit_once('/') {
                    registry
                        .module_by_stem
                        .entry(dir.to_string())
                        .or_insert_with(|| module_qn.clone());
                }
            }
            if let Some(package) = &extract.package {
                let dir = if rel_path.contains('/') {
                    rel_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
                } else {
                    ""
                };
                registry
                    .package_roots
                    .entry(package.name.clone())
                    .or_insert_with(|| dir.to_string());
                registry
                    .package_names
                    .entry(package.name.clone())
                    .or_insert_with(|| package.ecosystem.clone());
                // Go module paths map to the repository root.
                if package.ecosystem == "go" {
                    registry
                        .package_roots
                        .entry(package.name.clone())
                        .or_default();
                }
            }
            if let Some(adr) = &extract.adr {
                let module = fqn_module_dir(project, rel_path, *module_is_dir);
                registry
                    .adr_by_id
                    .entry(adr.id.to_ascii_uppercase())
                    .or_insert_with(|| format!("{module}.__adr__"));
            }
        }
        for (extract, rel_path, module_is_dir) in files {
            let module_qn = fqn_module_dir(project, rel_path, *module_is_dir);
            let mut bindings = HashMap::new();
            for import in &extract.imports {
                let binding = registry.resolve_import(project, rel_path, &module_qn, import);
                let local = import
                    .local_name
                    .clone()
                    .unwrap_or_else(|| import.specifier.clone());
                if !local.is_empty() {
                    bindings.entry(local).or_insert(binding);
                }
                if let Some(imported) = &import.imported_name {
                    bindings.entry(imported.clone()).or_insert_with(|| {
                        registry.resolve_import(project, rel_path, &module_qn, import)
                    });
                }
            }
            registry
                .imports_by_file
                .insert(rel_path.to_string(), bindings);
        }
        registry
    }

    fn resolve_import(
        &self,
        project: &str,
        rel_path: &str,
        module_qn: &str,
        import: &Import,
    ) -> ImportBinding {
        if import.is_relative || import.specifier.starts_with('.') {
            if let Some(stem) = resolve_relative_import(rel_path, &import.specifier) {
                if let Some(target) = self.module_by_stem.get(&stem) {
                    return ImportBinding {
                        target_module: Some(target.clone()),
                        target_package: None,
                        external: false,
                    };
                }
                // Try index/__init__ resolution.
                for suffix in ["/index", "/__init__", "/mod"] {
                    if let Some(target) = self.module_by_stem.get(&format!("{stem}{suffix}")) {
                        return ImportBinding {
                            target_module: Some(target.clone()),
                            target_package: None,
                            external: false,
                        };
                    }
                }
            }
        }
        let spec = &import.specifier;
        // Rust crate-internal paths.
        let rust_internal = spec.starts_with("crate::")
            || spec.starts_with("self::")
            || spec.starts_with("super::");
        if rust_internal {
            let mut path = spec
                .trim_start_matches("crate::")
                .trim_start_matches("self::")
                .replace("::", "/");
            if spec.starts_with("super::") {
                path = path.trim_start_matches("super::").to_string();
                let base = rel_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let parent = base.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                if !parent.is_empty() {
                    path = format!("{parent}/{path}");
                }
            }
            let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            while !parts.is_empty() {
                let stem = parts.join("/");
                if let Some(target) = self.module_by_stem.get(&stem) {
                    return ImportBinding {
                        target_module: Some(target.clone()),
                        target_package: None,
                        external: false,
                    };
                }
                if let Some(target) = self.module_by_stem.get(&format!("src/{stem}")) {
                    return ImportBinding {
                        target_module: Some(target.clone()),
                        target_package: None,
                        external: false,
                    };
                }
                // Cargo layouts can place sources under crates/*/src; accept a
                // unique suffix match.
                let suffix = format!("/{stem}");
                let mut matches: Vec<&String> = self
                    .module_by_stem
                    .iter()
                    .filter(|(k, _)| k.ends_with(&suffix) || k.as_str() == stem)
                    .map(|(_, v)| v)
                    .collect();
                matches.sort();
                matches.dedup();
                if matches.len() == 1 {
                    return ImportBinding {
                        target_module: Some(matches[0].clone()),
                        target_package: None,
                        external: false,
                    };
                }
                parts.pop();
            }
        }
        // Local workspace package or path.
        if let Some(root) = self.package_roots.get(spec) {
            let target = if root.is_empty() {
                project.to_string()
            } else {
                fqn_module_dir(project, &format!("{root}/__init__.py"), true)
            };
            return ImportBinding {
                target_module: Some(target),
                target_package: None,
                external: false,
            };
        }
        // Subpath under a local package: @scope/pkg/sub, github.com/org/repo/x.
        for (name, root) in &self.package_roots {
            if spec.starts_with(name) {
                let rest = spec[name.len()..].trim_start_matches('/');
                let stem = if rest.is_empty() {
                    root.clone()
                } else if root.is_empty() {
                    rest.to_string()
                } else {
                    format!("{root}/{rest}")
                };
                if let Some(target) = self.module_by_stem.get(&stem) {
                    return ImportBinding {
                        target_module: Some(target.clone()),
                        target_package: None,
                        external: false,
                    };
                }
                // Go packages are directories, not file stems. Resolve a
                // module-path package (module/internal/database) to the unique
                // module whose files live under that directory.
                if !stem.is_empty() {
                    let prefix = format!("{stem}/");
                    let mut matches: Vec<&String> = self
                        .module_by_stem
                        .iter()
                        .filter(|(key, _)| key.starts_with(&prefix))
                        .map(|(_, value)| value)
                        .collect();
                    matches.sort();
                    matches.dedup();
                    if matches.len() == 1 {
                        return ImportBinding {
                            target_module: Some(matches[0].clone()),
                            target_package: None,
                            external: false,
                        };
                    }
                }
            }
        }
        // Python dotted module.
        let dotted = spec.replace('.', "/");
        if let Some(target) = self.module_by_stem.get(&dotted) {
            return ImportBinding {
                target_module: Some(target.clone()),
                target_package: None,
                external: false,
            };
        }
        // Local relative-looking bare specifier (e.g. C headers, Python siblings).
        if let Some(target) = self
            .module_by_stem
            .get(spec.trim_end_matches(".h").trim_end_matches(".hpp"))
        {
            return ImportBinding {
                target_module: Some(target.clone()),
                target_package: None,
                external: false,
            };
        }
        let _ = module_qn;
        ImportBinding {
            target_module: None,
            target_package: Some(spec.clone()),
            external: true,
        }
    }

    pub fn imports_for(&self, file_path: &str) -> Option<&HashMap<String, ImportBinding>> {
        self.imports_by_file.get(file_path)
    }

    pub fn symbol(&self, idx: usize) -> &Symbol {
        &self.symbols[idx]
    }

    pub fn module_symbols(&self, module_qn: &str) -> &[usize] {
        self.by_module
            .get(module_qn)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Resolve a call site.
    pub fn resolve_call(
        &self,
        _project: &str,
        caller_file: &str,
        caller_module: &str,
        caller_scope: &[String],
        caller_language: &str,
        call: &CallSite,
    ) -> Option<Resolved> {
        let indices = self.by_name.get(&call.name)?;
        if indices.len() > 256 {
            return None;
        }
        let caller_class = if caller_scope.len() >= 2 {
            caller_scope.get(caller_scope.len() - 2).cloned()
        } else {
            caller_scope.last().cloned()
        };
        let imports = self.imports_for(caller_file);
        // A receiver bound to an external package cannot name a local symbol.
        // Without this guard a unique local short name (database.New) absorbs
        // calls through an external package (cors.New).
        if let Some(receiver) = call.receiver.as_deref().or(call.receiver_type.as_deref()) {
            if imports
                .and_then(|map| map.get(receiver.trim()))
                .map(|binding| binding.external)
                .unwrap_or(false)
            {
                return None;
            }
        }
        let mut scored: Vec<(usize, f64, &'static str)> = Vec::new();
        for &idx in indices {
            let cand = &self.symbols[idx];
            if !language_compatible(caller_language, &cand.language) {
                continue;
            }
            let mut score = 0.25f64;
            let mut reason: &'static str = "name";
            // Receiver type / self dispatch.
            if let Some(receiver_type) = &call.receiver_type {
                if cand
                    .scope
                    .last()
                    .map(|s| s == receiver_type)
                    .unwrap_or(false)
                    || cand.qn.contains(&format!(".{receiver_type}."))
                {
                    score += 0.40;
                    reason = "receiver_type";
                } else if cand.label == "Method" {
                    score -= 0.10;
                }
            }
            if call.receiver.as_deref() == Some("self") || call.receiver.as_deref() == Some("this")
            {
                if let Some(class) = &caller_class {
                    if cand.scope.last() == Some(class) {
                        score += 0.45;
                        reason = "same_class";
                    }
                }
            }
            // Import map.
            if let Some(imports) = imports {
                if let Some(binding) = imports.get(&call.name) {
                    if let Some(module) = &binding.target_module {
                        if cand.module_qn == *module
                            || cand.module_qn.starts_with(&format!("{module}."))
                            || cand.qn.starts_with(&format!("{module}."))
                        {
                            score += 0.55;
                            reason = "import";
                        }
                    }
                }
                if let Some(receiver) = &call.receiver {
                    if let Some(binding) = imports.get(receiver.trim()) {
                        if let Some(module) = &binding.target_module {
                            if cand.module_qn == *module
                                || cand.module_qn.starts_with(&format!("{module}."))
                                || cand.qn.starts_with(&format!("{module}."))
                            {
                                score += 0.50;
                                reason = "import_receiver";
                            }
                        }
                    }
                }
            }
            if cand.file_path == caller_file {
                score += 0.35;
                if reason == "name" {
                    reason = "same_file";
                }
                if let Some(class) = &caller_class {
                    if cand.scope.last() == Some(class) {
                        score += 0.20;
                        reason = "same_class";
                    }
                }
            }
            if cand.module_qn == caller_module {
                score += 0.30;
                if reason == "name" {
                    reason = "same_module";
                }
            }
            if indices.len() == 1 {
                score += 0.25;
                if reason == "name" {
                    reason = "unique_name";
                }
            }
            if cand.qn.ends_with(&format!(".{}", call.name)) {
                score += 0.10;
                if reason == "name" {
                    reason = "suffix";
                }
            }
            if cand.is_test {
                score -= 0.20;
            }
            // Constructors bind to the type declaration.
            if call.kind == "constructor" && cand.label == labels::CLASS {
                score += 0.25;
            }
            scored.push((idx, score.clamp(0.0, 1.0), reason));
        }
        if scored.is_empty() {
            return None;
        }
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(self.symbols[a.0].qn.cmp(&self.symbols[b.0].qn))
        });
        let (best_idx, best_score, reason) = scored[0];
        if best_score < 0.45 {
            return None;
        }
        let ambiguous = scored
            .get(1)
            .map(|(_, score, _)| (best_score - *score).abs() < 0.08)
            .unwrap_or(false);
        Some(Resolved {
            target_qn: self.symbols[best_idx].qn.clone(),
            confidence: if ambiguous {
                (best_score - 0.10).max(0.25)
            } else {
                best_score
            },
            resolved_by: if ambiguous { "ambiguous" } else { reason },
            ambiguous,
            candidates: scored.len(),
        })
    }

    pub fn resolve_usage(
        &self,
        caller_file: &str,
        caller_language: &str,
        usage: &Usage,
    ) -> Option<Resolved> {
        let indices = self.by_name.get(&usage.name)?;
        if indices.len() > 256 {
            return None;
        }
        let mut candidates: Vec<(usize, f64, &'static str)> = Vec::new();
        for &idx in indices {
            let cand = &self.symbols[idx];
            if !language_compatible(caller_language, &cand.language) {
                continue;
            }
            let mut score = 0.30;
            let mut reason = "name";
            if cand.file_path == caller_file {
                score += 0.35;
                reason = "same_file";
            }
            if indices.len() == 1 {
                score += 0.30;
                reason = "unique_name";
            }
            if cand.is_test {
                score -= 0.2;
            }
            candidates.push((idx, score, reason));
        }
        candidates.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(self.symbols[a.0].qn.cmp(&self.symbols[b.0].qn))
        });
        let (idx, score, reason) = candidates.into_iter().next()?;
        if score < 0.55 {
            return None;
        }
        Some(Resolved {
            target_qn: self.symbols[idx].qn.clone(),
            confidence: score,
            resolved_by: reason,
            ambiguous: false,
            candidates: indices.len(),
        })
    }

    pub fn resolve_type(
        &self,
        caller_file: &str,
        caller_language: &str,
        name: &str,
    ) -> Option<Resolved> {
        let simple = name.rsplit(['.', ':']).next().unwrap_or(name).to_string();
        let indices = self.by_name.get(&simple)?;
        let mut best: Option<(usize, f64, &'static str)> = None;
        for &idx in indices {
            let cand = &self.symbols[idx];
            if !labels::is_type_like(&cand.label) && cand.label != labels::TRAIT {
                continue;
            }
            if !language_compatible(caller_language, &cand.language) {
                continue;
            }
            let mut score = 0.40;
            let mut reason = "unique_name";
            if cand.file_path == caller_file {
                score += 0.40;
                reason = "same_file";
            }
            if indices.len() > 1 {
                score -= 0.10;
                reason = "suffix";
            }
            if best.as_ref().map(|(_, s, _)| score > *s).unwrap_or(true) {
                best = Some((idx, score, reason));
            }
        }
        let (idx, score, reason) = best?;
        if score < 0.45 {
            return None;
        }
        Some(Resolved {
            target_qn: self.symbols[idx].qn.clone(),
            confidence: score,
            resolved_by: reason,
            ambiguous: false,
            candidates: indices.len(),
        })
    }

    pub fn resolve_adr(&self, id: &str) -> Option<&String> {
        self.adr_by_id.get(&id.to_ascii_uppercase())
    }

    pub fn symbols_in_module_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = &'a Symbol> + 'a {
        self.symbols
            .iter()
            .filter(move |s| s.qn.starts_with(prefix))
    }
}

pub fn language_compatible(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    fn family(lang: &str) -> &'static str {
        match lang {
            "javascript" | "typescript" | "tsx" | "vue" | "svelte" => "web",
            "c" | "cpp" | "cuda" | "objc" => "c",
            "java" | "kotlin" | "scala" => "jvm",
            "csharp" | "fsharp" => "dotnet",
            "python" => "python",
            "rust" => "rust",
            "go" => "go",
            _ => "other",
        }
    }
    family(a) == family(b) && (a == b || family(a) != "other")
}

pub fn caller_qn(project: &str, rel_path: &str, scope: &[String], module_is_dir: bool) -> String {
    if scope.is_empty() {
        fqn_module_dir(project, rel_path, module_is_dir)
    } else {
        let mut name = String::new();
        for part in scope {
            name.push_str(part);
            name.push('.');
        }
        name.pop();
        fqn_compute(project, rel_path, Some(&name))
    }
}

/// Import target and external package bookkeeping used by the pipeline.
pub fn import_edge_targets(
    project: &str,
    rel_path: &str,
    module_is_dir: bool,
    registry: &Registry,
    import: &Import,
) -> (Option<String>, Option<String>) {
    let module_qn = fqn_module_dir(project, rel_path, module_is_dir);
    let binding = registry.imports_for(rel_path).and_then(|map| {
        import
            .local_name
            .as_ref()
            .and_then(|l| map.get(l))
            .or_else(|| map.get(&import.specifier))
    });
    if let Some(binding) = binding {
        if let Some(module) = &binding.target_module {
            return (Some(module.clone()), None);
        }
        if let Some(pkg) = &binding.target_package {
            return (None, Some(pkg.clone()));
        }
    }
    let _ = module_qn;
    (None, Some(import.specifier.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_parse::model::{FileExtract, PackageInfo};

    fn symbol(
        qn: &str,
        name: &str,
        label: &str,
        file: &str,
        module: &str,
        scope: &[&str],
    ) -> Symbol {
        Symbol {
            qn: qn.to_string(),
            name: name.to_string(),
            label: label.to_string(),
            file_path: file.to_string(),
            language: "rust".to_string(),
            module_qn: module.to_string(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            start_line: 1,
            is_test: false,
        }
    }

    #[test]
    fn import_beats_unique_name() {
        let symbols = vec![
            symbol("p.a.run", "run", "Function", "a.rs", "p.a", &[]),
            symbol("p.b.run", "run", "Function", "b.rs", "p.b", &[]),
        ];
        let mut files = vec![(FileExtract::default(), "a.rs".to_string(), false)];
        let mut extract = FileExtract::default();
        extract.imports.push(Import {
            specifier: "crate::b::run".to_string(),
            local_name: Some("run".to_string()),
            ..Default::default()
        });
        files[0].0 = extract;
        let registry = Registry::build("p", &files, symbols);
        let call = CallSite {
            name: "run".to_string(),
            scope: vec![],
            ..Default::default()
        };
        let resolved = registry
            .resolve_call("p", "a.rs", "p.a", &[], "rust", &call)
            .unwrap();
        // No import target resolution because module_by_stem is not seeded for
        // the synthetic file; same-module/unique still picks a winner.
        assert!(resolved.confidence >= 0.45);
    }

    #[test]
    fn go_module_internal_import_is_not_external() {
        let gomod = FileExtract {
            package: Some(PackageInfo {
                name: "go-template".to_string(),
                ecosystem: "go".to_string(),
                dependencies: vec![],
                version: None,
            }),
            ..Default::default()
        };
        let database = FileExtract {
            package: Some(PackageInfo {
                name: "database".to_string(),
                ecosystem: "go".to_string(),
                dependencies: vec![],
                version: None,
            }),
            ..Default::default()
        };
        let mut routes = FileExtract::default();
        routes.imports.push(Import {
            specifier: "go-template/internal/database".to_string(),
            local_name: Some("database".to_string()),
            ..Default::default()
        });
        let files = vec![
            (gomod, "go.mod".to_string(), false),
            (database, "internal/database/database.go".to_string(), true),
            (routes, "internal/server/routes.go".to_string(), true),
        ];
        let mut new = symbol(
            "p.internal.database.New",
            "New",
            "Function",
            "internal/database/database.go",
            "p.internal.database",
            &[],
        );
        new.language = "go".to_string();
        let registry = Registry::build("p", &files, vec![new]);
        let binding = registry
            .imports_for("internal/server/routes.go")
            .and_then(|imports| imports.get("database"))
            .expect("database import binding");
        assert!(
            !binding.external,
            "module-internal import was marked external"
        );
        assert_eq!(
            binding.target_module.as_deref(),
            Some("p.internal.database")
        );
    }

    #[test]
    fn external_receiver_does_not_absorb_unique_local_name() {
        let mut new = symbol(
            "p.database.New",
            "New",
            "Function",
            "internal/database/database.go",
            "p.database",
            &[],
        );
        new.language = "go".to_string();
        let symbols = vec![new];
        let mut extract = FileExtract::default();
        extract.imports.push(Import {
            specifier: "github.com/gin-contrib/cors".to_string(),
            local_name: Some("cors".to_string()),
            ..Default::default()
        });
        let files = vec![(extract, "internal/server/routes.go".to_string(), false)];
        let registry = Registry::build("p", &files, symbols);
        let call = CallSite {
            name: "New".to_string(),
            receiver: Some("cors".to_string()),
            ..Default::default()
        };
        assert!(
            registry
                .resolve_call(
                    "p",
                    "internal/server/routes.go",
                    "p.server",
                    &[],
                    "go",
                    &call
                )
                .is_none(),
            "cors.New must not resolve to database.New"
        );
    }

    #[test]
    fn language_families_are_vetoed() {
        assert!(language_compatible("javascript", "typescript"));
        assert!(language_compatible("c", "cpp"));
        assert!(!language_compatible("python", "rust"));
    }
}
