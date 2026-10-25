//! Qualified-name computation for the graph identity scheme.
//!
//! Format: `project.dir.parts.name`.
//! - File nodes (`name == "__file__"`) keep the full filename and extension.
//! - Other nodes strip the last extension.
//! - `__init__` / `index` are dropped from the last path segment when a symbol
//!   name is present (Python packages and JS barrel files).
//! - Modules for Java/Go are directory-based; this is selected by the caller.

/// Maximum project-name bytes, leaving headroom for database sidecar suffixes.
pub const MAX_PROJECT_NAME_LEN: usize = 200;

pub const FILE_QN_NAME: &str = "__file__";

fn is_build_file_variant(base: &str) -> bool {
    base.starts_with("Dockerfile.") || base.starts_with("Containerfile.")
}

fn strip_extension(path: &mut String) {
    let start = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &path[start..];
    if is_build_file_variant(base) {
        return;
    }
    if let Some(dot) = base.rfind('.') {
        if dot > 0 {
            path.truncate(start + dot);
        }
    }
}

fn segments(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty()).collect()
}

/// Compute `<project>.<path parts>.<name>`.
pub fn fqn_compute(project: &str, rel_path: &str, name: Option<&str>) -> String {
    if project.is_empty() {
        return String::new();
    }
    let is_file_qn = name == Some(FILE_QN_NAME);
    let mut path = rel_path.replace('\\', "/");
    if !is_file_qn {
        strip_extension(&mut path);
    }
    let mut parts: Vec<String> = Vec::new();
    parts.push(project.to_string());
    for segment in segments(&path) {
        parts.push(segment.to_string());
    }
    if let Some(name) = name {
        if !name.is_empty() {
            if parts.len() > 2 {
                let last = parts.last().map(String::as_str).unwrap_or("");
                if last == "__init__" || last == "index" {
                    parts.pop();
                }
            }
            parts.push(name.to_string());
        }
    }
    parts.join(".")
}

/// Module QN: `<project>.<path without extension>`.
pub fn fqn_module(project: &str, rel_path: &str) -> String {
    fqn_compute(project, rel_path, None)
}

/// Folder QN: `<project>.<dir parts>`.
pub fn fqn_folder(project: &str, rel_dir: &str) -> String {
    let rel_dir = rel_dir.replace('\\', "/");
    let rel_dir = rel_dir.trim_matches('/');
    if rel_dir.is_empty() {
        return project.to_string();
    }
    let mut parts = vec![project.to_string()];
    for segment in segments(rel_dir) {
        if segment == "." {
            continue;
        }
        parts.push(segment.to_string());
    }
    parts.join(".")
}

/// Directory-module QN for Java/Go packages.
pub fn fqn_module_dir(project: &str, rel_path: &str, module_is_dir: bool) -> String {
    if !module_is_dir {
        return fqn_module(project, rel_path);
    }
    match rel_path.rfind(['/', '\\']) {
        Some(idx) => fqn_folder(project, &rel_path[..idx]),
        None => project.to_string(),
    }
}

/// Project name derivation from an absolute path.
///
/// Unsafe ASCII becomes `-`, non-ASCII bytes become two lowercase hex digits,
/// runs of dashes/dots collapse, leading/trailing separators are trimmed, and
/// the result is capped at `MAX_PROJECT_NAME_LEN` bytes.
pub fn project_name_from_path(abs_path: &str) -> String {
    let trimmed = abs_path.trim_end_matches(['/', '\\']);
    let base = trimmed
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(trimmed);
    sanitize_project_name(base)
}

/// The name-mapping half of `project_name_from_path`, without canonicalization.
pub fn sanitize_project_name(name_path: &str) -> String {
    let mut out = String::with_capacity(name_path.len());
    let mut last_sep = false;
    for byte in name_path.bytes() {
        let ch = byte as char;
        if byte.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            if (ch == '-' || ch == '.') && (out.is_empty() || last_sep) {
                continue;
            }
            out.push(if ch == '.' { '-' } else { ch });
            last_sep = ch == '-' || ch == '.';
        } else if byte.is_ascii() {
            if !out.is_empty() && !last_sep {
                out.push('-');
                last_sep = true;
            }
        } else {
            out.push_str(&format!("{byte:02x}"));
            last_sep = false;
        }
    }
    while out.ends_with('-') || out.ends_with('.') {
        out.pop();
    }
    if out.len() > MAX_PROJECT_NAME_LEN {
        out.truncate(MAX_PROJECT_NAME_LEN);
    }
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

/// Resolve a relative import specifier against a source file path.
///
/// Returns a normalized repository-relative path without extension, or `None`
/// for bare module names (which are resolved through package manifests).
pub fn resolve_relative_import(source_rel: &str, module_path: &str) -> Option<String> {
    if module_path.is_empty() {
        return None;
    }
    let m = module_path.replace('\\', "/");
    let is_relative = m.starts_with("./")
        || m.starts_with("../")
        || m.starts_with('.')
        || (!m.contains('/')
            && (m.ends_with(".h")
                || m.ends_with(".hpp")
                || m.ends_with(".hh")
                || m.ends_with(".c")
                || m.ends_with(".cc")
                || m.ends_with(".cpp")
                || m.ends_with(".rs")
                || m.ends_with(".py")
                || m.ends_with(".js")
                || m.ends_with(".ts")));
    if !is_relative {
        return None;
    }
    let source_dir = source_rel
        .rfind('/')
        .map(|i| &source_rel[..i])
        .unwrap_or("");
    let mut stack: Vec<&str> = if source_dir.is_empty() {
        Vec::new()
    } else {
        source_dir.split('/').collect()
    };
    let mut parts = m.split('/');
    let first = parts.next().unwrap_or("");
    if first == "." || first.is_empty() {
        // current directory
    } else if first == ".." {
        stack.pop();
    } else if first.starts_with('.') {
        // Python relative import: `.foo` / `..foo`; leading dots are parents.
        let dots = first.chars().take_while(|c| *c == '.').count();
        for _ in 1..dots {
            stack.pop();
        }
        if first.len() > dots {
            stack.push(&first[dots..]);
        }
    } else {
        stack.push(first);
    }
    for part in parts {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    let mut result = stack.join("/");
    // Strip a known source extension, matching the module QN scheme.
    if let Some(dot) = result.rfind('.') {
        let ext = &result[dot..];
        if matches!(
            ext,
            ".h" | ".hh"
                | ".hpp"
                | ".hxx"
                | ".c"
                | ".cc"
                | ".cpp"
                | ".cxx"
                | ".m"
                | ".mm"
                | ".rs"
                | ".py"
                | ".js"
                | ".jsx"
                | ".mjs"
                | ".cjs"
                | ".ts"
                | ".tsx"
                | ".mts"
                | ".cts"
                | ".go"
                | ".java"
                | ".kt"
                | ".kts"
                | ".cs"
                | ".rb"
                | ".php"
                | ".swift"
                | ".scala"
                | ".lua"
                | ".sh"
                | ".bash"
                | ".ex"
                | ".exs"
                | ".hs"
                | ".ml"
                | ".mli"
                | ".dart"
                | ".R"
                | ".r"
                | ".jl"
                | ".zig"
                | ".nix"
                | ".tf"
                | ".tofu"
        ) {
            result.truncate(dot);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_symbol_qn() {
        assert_eq!(
            fqn_compute("proj", "src/api/handler.ts", Some("handle")),
            "proj.src.api.handler.handle"
        );
    }

    #[test]
    fn file_qn_keeps_extension_and_stem() {
        assert_eq!(
            fqn_compute("proj", "src/.env.local", Some(FILE_QN_NAME)),
            "proj.src..env.local.__file__"
        );
        assert_eq!(
            fqn_compute("proj", "a/foo.cpp", Some(FILE_QN_NAME)),
            "proj.a.foo.cpp.__file__"
        );
    }

    #[test]
    fn init_and_index_dropped_only_with_name() {
        assert_eq!(fqn_compute("p", "pkg/__init__.py", Some("f")), "p.pkg.f");
        assert_eq!(fqn_compute("p", "pkg/__init__.py", None), "p.pkg.__init__");
        assert_eq!(fqn_compute("p", "web/index.ts", Some("f")), "p.web.f");
        assert_eq!(fqn_compute("p", "web/index.ts", None), "p.web.index");
    }

    #[test]
    fn dockerfile_variants_keep_suffix() {
        assert_eq!(
            fqn_compute("p", "Dockerfile.debug", None),
            "p.Dockerfile.debug"
        );
    }

    #[test]
    fn module_dir_for_go_java() {
        assert_eq!(fqn_module_dir("p", "a/b/Foo.java", true), "p.a.b");
        assert_eq!(fqn_module_dir("p", "a/b/foo.go", true), "p.a.b");
        assert_eq!(fqn_module_dir("p", "a/b/foo.py", false), "p.a.b.foo");
    }

    #[test]
    fn project_names() {
        assert_eq!(project_name_from_path("/tmp/My Repo!"), "My-Repo");
        assert_eq!(project_name_from_path("/tmp/a--b..c"), "a-b-c");
        assert_eq!(sanitize_project_name("naïve"), "nac3afve");
    }

    #[test]
    fn relative_imports() {
        assert_eq!(
            resolve_relative_import("src/a/b.ts", "./c").as_deref(),
            Some("src/a/c")
        );
        assert_eq!(
            resolve_relative_import("src/a/b.py", "..pkg.mod").as_deref(),
            Some("src/pkg.mod")
        );
        assert_eq!(resolve_relative_import("src/a/b.ts", "lodash"), None);
    }
}
