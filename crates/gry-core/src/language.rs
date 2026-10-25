//! Language and file-format detection.
//!
//! The table covers the formats and grammar set compiled by gry. A file may be
//! recognised even when gry has no grammar for it: it still becomes a `File`
//! node and can be searched, but it is not parsed into symbols.

use once_cell::sync::Lazy;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageKind {
    Code,
    Documentation,
    Config,
    Infrastructure,
    Data,
}

#[derive(Debug, Clone, Copy)]
pub struct LanguageDef {
    pub name: &'static str,
    /// tree-sitter grammar name used by `gry-parse`; `None` means file-only.
    pub grammar: Option<&'static str>,
    pub kind: LanguageKind,
    /// Java/Go-style module identity is the containing directory.
    pub module_is_dir: bool,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    /// Case-sensitive extension check (R, Fortran upper-case variants).
    pub case_sensitive: bool,
}

macro_rules! lang {
    ($name:literal, $grammar:expr, $kind:ident, $dir:literal, $ext:expr, $files:expr) => {
        LanguageDef {
            name: $name,
            grammar: $grammar,
            kind: LanguageKind::$kind,
            module_is_dir: $dir,
            extensions: $ext,
            filenames: $files,
            case_sensitive: false,
        }
    };
}

pub static LANGUAGES: &[LanguageDef] = &[
    lang!("rust", Some("rust"), Code, false, &["rs"], &[]),
    lang!("python", Some("python"), Code, false, &["py", "pyi"], &[]),
    lang!(
        "javascript",
        Some("javascript"),
        Code,
        false,
        &["js", "jsx", "mjs", "cjs"],
        &[]
    ),
    lang!(
        "typescript",
        Some("typescript"),
        Code,
        false,
        &["ts", "mts", "cts"],
        &[]
    ),
    lang!("tsx", Some("tsx"), Code, false, &["tsx"], &[]),
    lang!("go", Some("go"), Code, true, &["go"], &[]),
    lang!("c", Some("c"), Code, false, &["c"], &[]),
    lang!(
        "cpp",
        Some("cpp"),
        Code,
        false,
        &["cc", "ccm", "cpp", "cppm", "cxx", "h", "hh", "hpp", "hxx", "ixx", "inl", "ipp"],
        &[]
    ),
    lang!("java", Some("java"), Code, true, &["java"], &[]),
    lang!(
        "csharp",
        Some("csharp"),
        Code,
        false,
        &["cs", "razor", "cshtml"],
        &[]
    ),
    lang!("objc", None, Code, false, &["mm"], &[]),
    lang!("matlab", None, Code, false, &["matlab", "mlx"], &[]),
    lang!("magma", None, Code, false, &["mag", "magma"], &[]),
    lang!(
        "ruby",
        Some("ruby"),
        Code,
        false,
        &["rb", "rake", "gemspec"],
        &[]
    ),
    lang!("php", Some("php"), Code, false, &["php", "phtml"], &[]),
    lang!("kotlin", Some("kotlin"), Code, false, &["kt", "kts"], &[]),
    lang!("swift", Some("swift"), Code, false, &["swift"], &[]),
    lang!("scala", Some("scala"), Code, false, &["scala", "sc"], &[]),
    lang!("lua", Some("lua"), Code, false, &["lua"], &[]),
    lang!("luau", Some("luau"), Code, false, &["luau"], &[]),
    lang!("bash", Some("bash"), Code, false, &["sh", "bash"], &[]),
    lang!(
        "zsh",
        None,
        Code,
        false,
        &["zsh"],
        &[".zshrc", ".zshenv", ".zprofile"]
    ),
    lang!("fish", None, Code, false, &["fish"], &[]),
    lang!("elixir", Some("elixir"), Code, false, &["ex", "exs"], &[]),
    lang!("haskell", Some("haskell"), Code, false, &["hs"], &[]),
    lang!("ocaml", Some("ocaml"), Code, false, &["ml", "mli"], &[]),
    lang!("dart", Some("dart"), Code, false, &["dart"], &[]),
    lang!("r", Some("r"), Code, false, &["r"], &[]),
    lang!("julia", Some("julia"), Code, false, &["jl"], &[]),
    lang!("zig", Some("zig"), Code, false, &["zig"], &[]),
    lang!("nix", Some("nix"), Code, false, &["nix"], &[]),
    lang!(
        "hcl",
        Some("hcl"),
        Config,
        false,
        &["hcl", "tf", "tofu", "tfvars"],
        &[]
    ),
    lang!("yaml", Some("yaml"), Config, false, &["yaml", "yml"], &[]),
    lang!("toml", Some("toml"), Config, false, &["toml"], &[]),
    lang!(
        "json",
        Some("json"),
        Data,
        false,
        &["json", "jsonc", "json5"],
        &[]
    ),
    lang!(
        "openapi",
        Some("json"),
        Data,
        false,
        &[],
        &["openapi.json", "swagger.json"]
    ),
    lang!(
        "markdown",
        Some("markdown"),
        Documentation,
        false,
        &["md", "mdx"],
        &[]
    ),
    lang!("rst", None, Documentation, false, &["rst"], &[]),
    lang!(
        "asciidoc",
        None,
        Documentation,
        false,
        &["adoc", "asciidoc"],
        &[]
    ),
    lang!("pdf", None, Documentation, false, &["pdf"], &[]),
    lang!(
        "dockerfile",
        Some("dockerfile"),
        Infrastructure,
        false,
        &["dockerfile"],
        &["Dockerfile", "Containerfile"]
    ),
    lang!(
        "make",
        Some("make"),
        Infrastructure,
        false,
        &["mk"],
        &["Makefile", "makefile", "GNUmakefile"]
    ),
    lang!(
        "cmake",
        Some("cmake"),
        Infrastructure,
        false,
        &["cmake"],
        &["CMakeLists.txt"]
    ),
    lang!("sql", Some("sql"), Code, false, &["sql"], &[]),
    lang!(
        "graphql",
        Some("graphql"),
        Code,
        false,
        &["graphql", "gql"],
        &[]
    ),
    lang!("html", Some("html"), Code, false, &["html", "htm"], &[]),
    lang!("css", Some("css"), Code, false, &["css"], &[]),
    lang!("scss", Some("css"), Code, false, &["scss", "sass"], &[]),
    lang!("vue", Some("vue"), Code, false, &["vue"], &[]),
    lang!("svelte", Some("svelte"), Code, false, &["svelte"], &[]),
    lang!("perl", None, Code, false, &["pl", "pm"], &[]),
    lang!(
        "groovy",
        Some("groovy"),
        Code,
        false,
        &["groovy", "gradle"],
        &[]
    ),
    lang!("erlang", Some("erlang"), Code, false, &["erl", "hrl"], &[]),
    lang!(
        "fsharp",
        Some("fsharp"),
        Code,
        false,
        &["fs", "fsi", "fsx"],
        &[]
    ),
    lang!("solidity", Some("solidity"), Code, false, &["sol"], &[]),
    lang!(
        "powershell",
        Some("powershell"),
        Code,
        false,
        &["ps1", "psm1"],
        &[]
    ),
    lang!("pascal", Some("pascal"), Code, false, &["pas", "dpr"], &[]),
    lang!("d", Some("d"), Code, false, &["d"], &[]),
    lang!(
        "verilog",
        Some("verilog"),
        Code,
        false,
        &["v", "sv", "svh", "vh"],
        &[]
    ),
    lang!("vhdl", Some("vhdl"), Code, false, &["vhd", "vhdl"], &[]),
    lang!("ada", Some("ada"), Code, false, &["adb", "ads"], &[]),
    lang!("commonlisp", None, Code, false, &["cl", "lisp", "lsp"], &[]),
    lang!("scheme", Some("scheme"), Code, false, &["scm", "ss"], &[]),
    lang!("elm", Some("elm"), Code, false, &["elm"], &[]),
    lang!(
        "fortran",
        Some("fortran"),
        Code,
        false,
        &["f", "f90", "f95", "f03", "f08", "F90", "F95", "F03", "F08"],
        &[]
    ),
    lang!("cobol", None, Code, false, &["cob", "cbl"], &[]),
    lang!("cuda", Some("cuda"), Code, false, &["cu", "cuh"], &[]),
    lang!("glsl", None, Code, false, &["glsl", "vert", "frag"], &[]),
    lang!("hlsl", None, Code, false, &["hlsl", "fx", "fxh"], &[]),
    lang!("wgsl", None, Code, false, &["wgsl"], &[]),
    lang!("vim", None, Code, false, &["vim", "vimrc"], &[".vimrc"]),
    lang!(
        "starlark",
        Some("starlark"),
        Code,
        false,
        &["bzl"],
        &["BUILD", "BUILD.bazel", "WORKSPACE", "WORKSPACE.bazel"]
    ),
    lang!(
        "ini",
        Some("ini"),
        Config,
        false,
        &["ini", "cfg", "conf"],
        &[]
    ),
    lang!(
        "properties",
        Some("properties"),
        Config,
        false,
        &["properties"],
        &[]
    ),
    lang!("dotenv", None, Config, false, &["env"], &[".env"]),
    lang!("diff", Some("diff"), Data, false, &["diff", "patch"], &[]),
    lang!("regex", Some("regex"), Data, false, &["regex"], &[]),
    lang!("gdscript", Some("gdscript"), Code, false, &["gd"], &[]),
    lang!("hare", None, Code, false, &["ha"], &[]),
    lang!("pony", None, Code, false, &["pony"], &[]),
    lang!("nickel", Some("nickel"), Config, false, &["ncl"], &[]),
    lang!("crystal", Some("crystal"), Code, false, &["cr"], &[]),
    lang!("qml", Some("qmljs"), Code, false, &["qml"], &[]),
    lang!(
        "jinja",
        Some("jinja2"),
        Code,
        false,
        &["j2", "jinja", "jinja2"],
        &[]
    ),
    lang!("kdl", None, Data, false, &["kdl"], &[]),
    lang!("prisma", Some("prisma"), Code, false, &["prisma"], &[]),
    lang!("thrift", None, Code, false, &["thrift"], &[]),
    lang!("capnp", None, Code, false, &["capnp"], &[]),
    lang!("protobuf", None, Code, false, &["proto"], &[]),
    lang!(
        "gitattributes",
        None,
        Config,
        false,
        &[],
        &[".gitattributes"]
    ),
    lang!("gitignore", None, Config, false, &[], &[".gitignore"]),
    lang!("csv", None, Data, false, &["csv", "tsv"], &[]),
    lang!(
        "xml",
        None,
        Data,
        false,
        &[
            "xml", "xsd", "xsl", "svg", "csproj", "vbproj", "fsproj", "props", "targets", "nuspec",
            "resx", "xaml", "axaml", "plist"
        ],
        &[]
    ),
    lang!("text", None, Data, false, &["txt", "log"], &[]),
    lang!(
        "requirements",
        None,
        Config,
        false,
        &[],
        &[
            "requirements.txt",
            "requirements-dev.txt",
            "requirements-test.txt"
        ]
    ),
    lang!("gomod", None, Config, false, &[], &["go.mod", "go.sum"]),
    lang!(
        "kustomize",
        Some("yaml"),
        Infrastructure,
        false,
        &[],
        &["kustomization.yaml", "kustomization.yml", "Kustomization"]
    ),
    lang!("kubernetes", Some("yaml"), Infrastructure, false, &[], &[]),
    lang!(
        "helm",
        Some("yaml"),
        Infrastructure,
        false,
        &[],
        &["Chart.yaml"]
    ),
    lang!(
        "compose",
        Some("yaml"),
        Infrastructure,
        false,
        &[],
        &[
            "docker-compose.yaml",
            "docker-compose.yml",
            "compose.yaml",
            "compose.yml"
        ]
    ),
    lang!(
        "github_actions",
        Some("yaml"),
        Infrastructure,
        false,
        &[],
        &[]
    ),
    lang!("ansible", Some("yaml"), Infrastructure, false, &[], &[]),
    lang!("dbt", Some("sql"), Code, false, &[], &[]),
    lang!("jupyter", None, Data, false, &["ipynb"], &[]),
    lang!(
        "binary",
        None,
        Data,
        false,
        &[
            "png", "jpg", "jpeg", "gif", "webp", "ico", "zip", "gz", "tgz", "zst", "bz2", "xz",
            "7z", "tar", "o", "a", "so", "dylib", "dll", "exe", "bin", "class", "jar", "war",
            "pyc", "pyo", "wasm", "ttf", "otf", "woff", "woff2", "mp3", "mp4", "mov", "avi", "db",
            "sqlite", "sqlite3"
        ],
        &[]
    ),
];

static BY_EXTENSION: Lazy<HashMap<&'static str, &'static LanguageDef>> = Lazy::new(|| {
    let mut map = HashMap::new();
    for lang in LANGUAGES {
        for ext in lang.extensions {
            map.entry(*ext).or_insert(lang);
        }
    }
    map
});

static BY_FILENAME: Lazy<HashMap<&'static str, &'static LanguageDef>> = Lazy::new(|| {
    let mut map = HashMap::new();
    for lang in LANGUAGES {
        for name in lang.filenames {
            map.entry(*name).or_insert(lang);
        }
    }
    map
});

fn language_by_name(name: &str) -> Option<&'static LanguageDef> {
    LANGUAGES.iter().find(|lang| lang.name == name)
}

/// Detect the language of a repository-relative path.
///
/// Compound and ambiguous forms are handled before the plain extension table:
/// `.blade.php`, `.d.ts`, `.env.*`, `Dockerfile.*`, `Containerfile.*`, `.m`,
/// `.h`, and `<name>.tfvars`.
pub fn detect(path: &str) -> Option<&'static LanguageDef> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = base.to_ascii_lowercase();

    if let Some(lang) = BY_FILENAME.get(base) {
        return Some(lang);
    }
    if base.starts_with(".env.") {
        return language_by_name("dotenv");
    }
    if (base.starts_with("Dockerfile.") || base.starts_with("Containerfile."))
        && !base.ends_with(".dockerignore")
    {
        return language_by_name("dockerfile");
    }
    if base.starts_with("docker-compose") && (lower.ends_with(".yml") || lower.ends_with(".yaml")) {
        return language_by_name("compose");
    }
    if lower.ends_with(".d.ts") {
        return language_by_name("typescript");
    }
    if lower.ends_with(".blade.php") {
        return language_by_name("php");
    }
    if base == ".m" || lower.ends_with(".m") {
        // Objective-C vs MATLAB/Magma is resolved from content by the caller;
        // default to MATLAB because .m source files dominate.
        return language_by_name("matlab").or_else(|| language_by_name("c"));
    }
    if lower.ends_with(".inc") {
        return language_by_name("php");
    }

    // Special extension handling.
    let ext = match base.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => return None,
    };
    if let Some(lang) = BY_EXTENSION.get(ext) {
        return Some(lang);
    }
    // Case-insensitive match for extensions that differ only by case.
    let lower_ext = ext.to_ascii_lowercase();
    if let Some(lang) = BY_EXTENSION.get(lower_ext.as_str()) {
        return Some(lang);
    }
    None
}

/// Content-assisted disambiguation for `.m` (MATLAB vs Objective-C) and `.h`
/// (C vs C++). Returns the initial detection when no stronger signal exists.
pub fn disambiguate(path: &str, content: &[u8]) -> Option<&'static LanguageDef> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if base == ".m" || base.ends_with(".m") {
        let text = String::from_utf8_lossy(&content[..content.len().min(8192)]);
        if text.contains("@interface")
            || text.contains("@implementation")
            || text.contains("@property")
            || text.contains("#import")
        {
            return language_by_name("objc").or_else(|| language_by_name("c"));
        }
        return language_by_name("matlab");
    }
    if base.ends_with(".h") || base.ends_with(".hpp") {
        // Headers are parsed with the C++ grammar, a superset for extraction
        // purposes, so keep that behavior.
        return language_by_name("cpp");
    }
    detect(path)
}

impl LanguageDef {
    pub fn is_code(&self) -> bool {
        self.kind == LanguageKind::Code
    }

    pub fn is_doc(&self) -> bool {
        self.kind == LanguageKind::Documentation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_core_languages() {
        assert_eq!(detect("src/main.rs").unwrap().name, "rust");
        assert_eq!(detect("app/views.py").unwrap().name, "python");
        assert_eq!(detect("web/index.tsx").unwrap().name, "tsx");
        assert_eq!(detect("web/index.ts").unwrap().name, "typescript");
        assert_eq!(detect("web/types.d.ts").unwrap().name, "typescript");
        assert_eq!(detect("Dockerfile.debug").unwrap().name, "dockerfile");
        assert_eq!(detect("Containerfile").unwrap().name, "dockerfile");
        assert_eq!(detect(".env.local").unwrap().name, "dotenv");
        assert_eq!(detect("pkg/__init__.py").unwrap().name, "python");
        assert_eq!(detect("Makefile").unwrap().name, "make");
        assert_eq!(detect("chart/Chart.yaml").unwrap().name, "helm");
        assert_eq!(
            detect("deploy/kustomization.yaml").unwrap().name,
            "kustomize"
        );
        assert_eq!(detect("compose.yml").unwrap().name, "compose");
    }

    #[test]
    fn unknown_and_binary() {
        assert!(detect("README").is_none());
        assert_eq!(detect("image.png").unwrap().name, "binary");
    }

    #[test]
    fn m_disambiguation() {
        assert_eq!(
            disambiguate("a.m", b"#import <Foundation/Foundation.h>")
                .unwrap()
                .name,
            "objc"
        );
        assert_eq!(
            disambiguate("a.m", b"function y = f(x)\ny=x;")
                .unwrap()
                .name,
            "matlab"
        );
        assert_eq!(
            disambiguate("a.h", b"namespace x { class A {}; }")
                .unwrap()
                .name,
            "cpp"
        );
        assert_eq!(disambiguate("a.h", b"int f(void);").unwrap().name, "cpp");
    }
}
