//! Deterministic repository discovery with layered ignore rules.
//!
//! Layers (first rejecting layer wins, with the documented exceptions):
//! built-in skip lists → repository/nested `.gitignore` → global git excludes →
//! `.gryignore`. `.gryignore` whitelist entries may re-include ordinary built-in
//! directories, but never the safety core (`.git`, `node_modules`, worktrees).

use gry_core::language;
use gry_core::types::FileHash;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexMode {
    Full,
    Moderate,
    Fast,
    CrossRepoIntelligence,
}

impl IndexMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "full" | "" => Some(IndexMode::Full),
            "moderate" => Some(IndexMode::Moderate),
            "fast" => Some(IndexMode::Fast),
            "cross-repo-intelligence" | "cross_repo" | "cross-repo" => {
                Some(IndexMode::CrossRepoIntelligence)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            IndexMode::Full => "full",
            IndexMode::Moderate => "moderate",
            IndexMode::Fast => "fast",
            IndexMode::CrossRepoIntelligence => "cross-repo-intelligence",
        }
    }

    pub fn semantic_edges(self) -> bool {
        matches!(self, IndexMode::Full | IndexMode::Moderate)
    }

    pub fn filtered_discovery(self) -> bool {
        !matches!(self, IndexMode::Full)
    }

    pub fn doc_candidates(self, setting: Option<bool>) -> bool {
        setting.unwrap_or(self != IndexMode::Fast)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverOptions {
    pub mode: IndexMode,
    pub max_files: Option<u64>,
    pub max_source_mb: Option<u64>,
    pub max_file_bytes: u64,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            mode: IndexMode::Full,
            max_files: None,
            max_source_mb: None,
            max_file_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredFile {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub size: i64,
    pub mtime_ns: i64,
    pub sha256: String,
    pub language: String,
    pub grammar: Option<String>,
    pub module_is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoredFile {
    pub rel_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Discovery {
    pub root: PathBuf,
    pub files: Vec<DiscoveredFile>,
    pub excluded_dirs: Vec<String>,
    pub ignored_files: Vec<IgnoredFile>,
    pub source_bytes: i64,
    pub limit_violation: Option<String>,
    pub coverage: Vec<CoverageNote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageNote {
    pub rel_path: String,
    pub kind: String,
    pub detail: String,
}

const SAFETY_CORE: &[&str] = &[".git", "node_modules", ".worktrees", ".claude-worktrees"];
const BUILTIN_DIRS: &[&str] = &[
    "target",
    "dist",
    "build",
    "out",
    "bin",
    "obj",
    "vendor",
    "third_party",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".cache",
    "coverage",
    "htmlcov",
    ".next",
    ".nuxt",
    ".turbo",
    ".parcel-cache",
    ".gradle",
    ".idea",
    ".vscode",
    ".terraform",
    ".terragrunt-cache",
    ".serverless",
    ".aws-sam",
    ".svelte-kit",
    ".astro",
    ".angular",
    ".dart_tool",
    "Pods",
    "DerivedData",
    "Carthage",
    "buck-out",
    ".bundle",
    ".yarn",
    ".pnpm-store",
    ".hg",
    ".svn",
    ".vs",
    "TestResults",
    ".fleet",
    ".history",
    ".nyc_output",
    ".coverage",
];

const FAST_EXTRA_DIRS: &[&str] = &[
    "docs",
    "doc",
    "examples",
    "example",
    "samples",
    "sample",
    "testdata",
    "fixtures",
    "benchmarks",
    "benchmark",
    "bench",
    "demo",
    "demos",
    "scripts",
];

fn builtin_dir(name: &str, mode: IndexMode) -> bool {
    if BUILTIN_DIRS.contains(&name) {
        return true;
    }
    if name.starts_with("bazel-") || name.starts_with(".bazel") {
        return true;
    }
    if mode.filtered_discovery() && FAST_EXTRA_DIRS.contains(&name) {
        return true;
    }
    false
}

fn ignored_suffix(name: &str, mode: IndexMode) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let binary = [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".bmp", ".tiff", ".psd", ".zip", ".gz",
        ".tgz", ".bz2", ".xz", ".7z", ".tar", ".zst", ".rar", ".o", ".a", ".so", ".dylib", ".dll",
        ".exe", ".bin", ".class", ".jar", ".war", ".pyc", ".pyo", ".wasm", ".ttf", ".otf", ".woff",
        ".woff2", ".eot", ".mp3", ".mp4", ".mov", ".avi", ".mkv", ".wav", ".flac", ".db",
        ".sqlite", ".sqlite3", ".mdb", ".iso", ".dmg", ".pkg", ".deb", ".rpm",
    ];
    if binary.iter().any(|s| lower.ends_with(s)) {
        return Some("binary");
    }
    if matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini") {
        return Some("metadata");
    }
    if mode.filtered_discovery() {
        let extra = [
            ".map", ".lock", ".min.js", ".min.css", ".log", ".tmp", ".swp", ".bak", ".orig",
        ];
        if extra.iter().any(|s| lower.ends_with(s)) {
            return Some("filtered");
        }
    }
    None
}

fn is_generated_dir_suffix(rel: &str) -> bool {
    rel.ends_with("storage/framework/views")
}

fn is_skipped_filename(name: &str) -> bool {
    matches!(
        name,
        ".DS_Store" | "Thumbs.db" | "desktop.ini" | ".npmrc" | ".yarnrc"
    )
}

/// Discover files, reusing known hashes when mtime and size are unchanged.
pub fn discover(
    root: &Path,
    options: &DiscoverOptions,
    known_hashes: &HashMap<String, FileHash>,
    cancel: &std::sync::atomic::AtomicBool,
) -> std::io::Result<Discovery> {
    let root = root.to_path_buf();
    let mut files = Vec::new();
    let mut ignored_files = Vec::new();
    let mut coverage = Vec::new();
    let mut source_bytes = 0i64;
    let mut limit_violation = None;

    let mut rules: Vec<(PathBuf, Gitignore)> = Vec::new();
    let root_gitignore = build_gitignore(&root, &root.join(".gitignore"));
    if let Some(matcher) = root_gitignore {
        rules.push((PathBuf::new(), matcher));
    }
    // Pre-collect nested .gitignore rules so the walk predicate can borrow the
    // rule list immutably. Skip safety-core and built-in directories here.
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if SAFETY_CORE.contains(&name.as_ref()) {
                return false;
            }
            !(e.file_type().is_dir() && builtin_dir(&name, options.mode))
        })
        .flatten()
    {
        if entry.file_type().is_file() && entry.file_name() == ".gitignore" {
            let rel_dir = entry
                .path()
                .parent()
                .map(|p| relative(&root, p))
                .unwrap_or_default();
            if let Some(matcher) = build_gitignore(&root, entry.path()) {
                rules.push((PathBuf::from(rel_dir), matcher));
            }
        }
    }
    let global = global_gitignore();
    let gryignore = build_gitignore(&root, &root.join(".gryignore"));
    let excluded_cell = std::cell::RefCell::new(Vec::<String>::new());

    let walker = WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            if entry.file_type().is_symlink() {
                return false;
            }
            if entry.file_type().is_dir() {
                let rel = relative(&root, entry.path());
                if SAFETY_CORE.contains(&name.as_ref()) {
                    return false;
                }
                if is_generated_dir_suffix(&rel) {
                    return false;
                }
                let gry_decision = gryignore
                    .as_ref()
                    .map(|g| g.matched_path_or_any_parents(&rel, true))
                    .unwrap_or(Match::None);
                if gry_decision.is_whitelist() {
                    return true;
                }
                let rejected = builtin_dir(&name, options.mode)
                    || gry_decision.is_ignore()
                    || git_ignored(&rules, &root, &rel, true)
                    || global
                        .as_ref()
                        .map(|g| g.matched_path_or_any_parents(&rel, true).is_ignore())
                        .unwrap_or(false);
                if rejected {
                    let mut excluded = excluded_cell.borrow_mut();
                    if excluded.len() < 25 {
                        excluded.push(rel.clone());
                    }
                    return false;
                }
                return true;
            }
            true
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                if let Some(path) = err.path() {
                    coverage.push(CoverageNote {
                        rel_path: relative(&root, path),
                        kind: "read".to_string(),
                        detail: err.to_string(),
                    });
                }
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = relative(&root, path);
        let name = entry.file_name().to_string_lossy().to_string();
        if is_skipped_filename(&name) {
            ignored_files.push(IgnoredFile {
                rel_path: rel,
                reason: "skip_list".to_string(),
            });
            continue;
        }
        if let Some(reason) = ignored_suffix(&name, options.mode) {
            ignored_files.push(IgnoredFile {
                rel_path: rel,
                reason: reason.to_string(),
            });
            continue;
        }
        let gry_decision = gryignore
            .as_ref()
            .map(|g| g.matched_path_or_any_parents(&rel, false))
            .unwrap_or(Match::None);
        if gry_decision.is_ignore()
            || git_ignored(&rules, &root, &rel, false)
            || (global
                .as_ref()
                .map(|g| g.matched_path_or_any_parents(&rel, false).is_ignore())
                .unwrap_or(false)
                && !gry_decision.is_whitelist())
        {
            ignored_files.push(IgnoredFile {
                rel_path: rel,
                reason: "ignore_rule".to_string(),
            });
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                coverage.push(CoverageNote {
                    rel_path: rel,
                    kind: "read".to_string(),
                    detail: e.to_string(),
                });
                continue;
            }
        };
        if metadata.len() > options.max_file_bytes {
            coverage.push(CoverageNote {
                rel_path: rel.clone(),
                kind: "oversized".to_string(),
                detail: format!(
                    "{} bytes > {} bytes",
                    metadata.len(),
                    options.max_file_bytes
                ),
            });
            continue;
        }
        let Some(lang) = language::detect(&rel) else {
            ignored_files.push(IgnoredFile {
                rel_path: rel,
                reason: "unknown_language".to_string(),
            });
            continue;
        };
        // Read only what is needed for content disambiguation/hashing.
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                coverage.push(CoverageNote {
                    rel_path: rel,
                    kind: "read".to_string(),
                    detail: e.to_string(),
                });
                continue;
            }
        };
        let lang = if matches!(lang.name, "matlab" | "c" | "cpp") {
            language::disambiguate(&rel, &bytes).unwrap_or(lang)
        } else {
            lang
        };
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let size = metadata.len() as i64;
        let sha256 = match known_hashes.get(&rel) {
            Some(known) if known.mtime_ns == mtime_ns && known.size == size => known.sha256.clone(),
            _ => sha256(&bytes),
        };
        source_bytes += size;
        files.push(DiscoveredFile {
            rel_path: rel,
            abs_path: path.to_path_buf(),
            size,
            mtime_ns,
            sha256,
            language: lang.name.to_string(),
            grammar: lang.grammar.map(str::to_string),
            module_is_dir: lang.module_is_dir,
        });
        if files.len() > 5_000_000 {
            limit_violation = Some("file count exceeded hard scan limit".to_string());
            break;
        }
    }

    // Pop nested rules that are no longer in scope (walker is depth-first, so a
    // flat list with base-prefix matching is equivalent for our purposes).

    let mut excluded_dirs = excluded_cell.into_inner();
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    ignored_files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    excluded_dirs.sort();
    excluded_dirs.dedup();

    if let Some(max) = options.max_files {
        if files.len() as u64 > max {
            limit_violation = Some(format!(
                "index_max_files: {} files discovered, limit {}",
                files.len(),
                max
            ));
        }
    }
    if let Some(max_mb) = options.max_source_mb {
        if source_bytes > (max_mb * 1024 * 1024) as i64 {
            limit_violation = Some(format!(
                "index_max_source_mb: {} MiB discovered, limit {} MiB",
                source_bytes / (1024 * 1024),
                max_mb
            ));
        }
    }

    // Load nested ignore rules for top-level directory entries as well, so a
    // file at depth 1 sees its directory's .gitignore.
    Ok(Discovery {
        root,
        files,
        excluded_dirs,
        ignored_files,
        source_bytes,
        limit_violation,
        coverage,
    })
}

fn build_gitignore(base: &Path, file: &Path) -> Option<Gitignore> {
    if !file.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(base);
    if builder.add(file).is_some() {
        return None;
    }
    builder.build().ok()
}

fn global_gitignore() -> Option<Gitignore> {
    let home = std::env::var_os("HOME")?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    let path = xdg.join("git").join("ignore");
    if !path.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new("/");
    if builder.add(&path).is_some() {
        return None;
    }
    builder.build().ok()
}

fn git_ignored(rules: &[(PathBuf, Gitignore)], root: &Path, rel: &str, is_dir: bool) -> bool {
    let path = root.join(rel);
    for (base, matcher) in rules.iter().rev() {
        let candidate = if base.as_os_str().is_empty() {
            path.as_path()
        } else {
            // `matched_path_or_any_parents` expects an absolute or base-relative
            // path; strip the rule's base prefix.
            match path.strip_prefix(root.join(base)) {
                Ok(p) => p,
                Err(_) => continue,
            }
        };
        let matched = matcher.matched_path_or_any_parents(candidate, is_dir);
        if matched.is_ignore() {
            return true;
        }
        if matched.is_whitelist() {
            return false;
        }
    }
    false
}

pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Discover candidate test files by conventions, used by test-impact.
pub fn test_files(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = relative(root, entry.path());
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("test_")
                || name.ends_with("_test.c")
                || name.ends_with("_test.cpp")
                || name.ends_with("_test.go")
                || name.ends_with("_test.py")
                || name.ends_with("_test.rs")
                || name.ends_with(".test.ts")
                || name.ends_with(".spec.ts")
                || rel.contains("/tests/")
            {
                out.insert(rel);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn layered_ignores_and_safety_core() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".gitignore", "*.log\nbuild/\n");
        write(dir.path(), ".gryignore", "secret.rs\n");
        write(dir.path(), "src/main.rs", "fn main(){}");
        write(dir.path(), "src/secret.rs", "fn secret(){}");
        write(dir.path(), "debug.log", "x");
        write(dir.path(), "build/out.rs", "fn x(){}");
        write(dir.path(), "node_modules/pkg/a.js", "x");
        write(dir.path(), "target/x.rs", "fn x(){}");
        let found = discover(
            dir.path(),
            &DiscoverOptions::default(),
            &HashMap::new(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let paths: Vec<&str> = found.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert!(paths.contains(&"src/main.rs"));
        assert!(!paths.iter().any(|p| p.contains("secret")
            || p.ends_with(".log")
            || p.starts_with("build/")
            || p.starts_with("target/")
            || p.starts_with("node_modules/")));
    }

    #[test]
    fn gryignore_can_reinclude_builtin_but_not_safety_core() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".gryignore", "!target/\n!node_modules/\n");
        write(dir.path(), "target/keep.rs", "fn keep(){}");
        write(dir.path(), "node_modules/x/a.js", "x");
        write(dir.path(), "src/main.rs", "fn main(){}");
        let found = discover(
            dir.path(),
            &DiscoverOptions::default(),
            &HashMap::new(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let paths: Vec<&str> = found.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert!(paths.contains(&"target/keep.rs"));
        assert!(!paths.iter().any(|p| p.starts_with("node_modules")));
    }

    #[test]
    fn hashes_are_reused_when_mtime_and_size_match() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/main.rs", "fn main(){}");
        let first = discover(
            dir.path(),
            &DiscoverOptions::default(),
            &HashMap::new(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let known: HashMap<String, FileHash> = first
            .files
            .iter()
            .map(|f| {
                (
                    f.rel_path.clone(),
                    FileHash {
                        rel_path: f.rel_path.clone(),
                        sha256: f.sha256.clone(),
                        mtime_ns: f.mtime_ns,
                        size: f.size,
                    },
                )
            })
            .collect();
        let second = discover(
            dir.path(),
            &DiscoverOptions::default(),
            &known,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(second.files[0].sha256, first.files[0].sha256);
    }

    #[test]
    fn max_files_fails_the_request() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a(){}");
        write(dir.path(), "b.rs", "fn b(){}");
        let found = discover(
            dir.path(),
            &DiscoverOptions {
                max_files: Some(1),
                ..Default::default()
            },
            &HashMap::new(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(found.limit_violation.is_some());
    }

    #[test]
    fn filtered_mode_skips_docs() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/main.rs", "fn main(){}");
        write(dir.path(), "docs/guide.md", "# Guide\n");
        let found = discover(
            dir.path(),
            &DiscoverOptions {
                mode: IndexMode::Fast,
                ..Default::default()
            },
            &HashMap::new(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(found.files.len(), 1);
    }
}
