//! Statically linked tree-sitter grammar registry.
//!
//! Grammars are compiled into the binary (no runtime download). The list is a
//! deliberate superset of the languages gry extracts semantically; formats
//! without a compatible modern grammar fall back to the generic extractor.

use tree_sitter::Language;

/// Resolve a grammar name to a tree-sitter language.
pub fn grammar(name: &str) -> Option<Language> {
    let language = match name {
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" => tree_sitter_cpp::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "csharp" => tree_sitter_c_sharp::LANGUAGE.into(),
        "ruby" => tree_sitter_ruby::LANGUAGE.into(),
        "php" => tree_sitter_php::LANGUAGE_PHP.into(),
        "kotlin" => tree_sitter_kt::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "scala" => tree_sitter_scala::LANGUAGE.into(),
        "lua" => tree_sitter_lua::LANGUAGE.into(),
        "bash" => tree_sitter_bash::LANGUAGE.into(),
        "elixir" => tree_sitter_elixir::LANGUAGE.into(),
        "haskell" => tree_sitter_haskell::LANGUAGE.into(),
        "ocaml" => tree_sitter_ocaml::LANGUAGE_OCAML.into(),
        "dart" => tree_sitter_dart::LANGUAGE.into(),
        "r" => tree_sitter_r::LANGUAGE.into(),
        "julia" => tree_sitter_julia::LANGUAGE.into(),
        "zig" => tree_sitter_zig::LANGUAGE.into(),
        "nix" => tree_sitter_nix::LANGUAGE.into(),
        "hcl" => tree_sitter_hcl::LANGUAGE.into(),
        "yaml" => tree_sitter_yaml::LANGUAGE.into(),
        "json" => tree_sitter_json::LANGUAGE.into(),
        "markdown" => tree_sitter_md_025::LANGUAGE.into(),
        "dockerfile" => tree_sitter_containerfile::LANGUAGE.into(),
        "make" => tree_sitter_make::LANGUAGE.into(),
        "cmake" => tree_sitter_cmake::LANGUAGE.into(),
        "sql" => tree_sitter_sequel::LANGUAGE.into(),
        "graphql" => tree_sitter_graphql::LANGUAGE.into(),
        "html" => tree_sitter_html::LANGUAGE.into(),
        "css" => tree_sitter_css::LANGUAGE.into(),
        "scss" => tree_sitter_scss::language(),
        "vue" => tree_sitter_vue_next::LANGUAGE.into(),
        "svelte" => tree_sitter_svelte_ng::LANGUAGE.into(),
        "groovy" => tree_sitter_groovy::LANGUAGE.into(),
        "erlang" => tree_sitter_erlang::LANGUAGE.into(),
        "fsharp" => tree_sitter_fsharp::LANGUAGE_FSHARP.into(),
        "solidity" => tree_sitter_solidity::LANGUAGE.into(),
        "powershell" => tree_sitter_powershell::LANGUAGE.into(),
        "pascal" => tree_sitter_pascal::LANGUAGE.into(),
        "d" => tree_sitter_d::LANGUAGE.into(),
        "verilog" => tree_sitter_verilog::LANGUAGE.into(),
        "vhdl" => tree_sitter_vhdl::LANGUAGE.into(),
        "ada" => tree_sitter_ada::LANGUAGE.into(),
        "commonlisp" => tree_sitter_commonlisp::LANGUAGE_COMMONLISP.into(),
        "scheme" => tree_sitter_scheme::LANGUAGE.into(),
        "elm" => tree_sitter_elm::LANGUAGE.into(),
        "fortran" => tree_sitter_fortran::LANGUAGE.into(),
        "cuda" => tree_sitter_cuda::LANGUAGE.into(),
        "glsl" => tree_sitter_glsl::LANGUAGE_GLSL.into(),
        "starlark" => tree_sitter_starlark::LANGUAGE.into(),
        "ini" => tree_sitter_ini::LANGUAGE.into(),
        "diff" => tree_sitter_diff::LANGUAGE.into(),
        "regex" => tree_sitter_regex::LANGUAGE.into(),
        "gdscript" => tree_sitter_gdscript::LANGUAGE.into(),
        "luau" => tree_sitter_luau::LANGUAGE.into(),
        "nickel" => tree_sitter_nickel::LANGUAGE.into(),
        "crystal" => tree_sitter_crystal::LANGUAGE.into(),
        "qmljs" => tree_sitter_qmljs::LANGUAGE.into(),
        "jinja2" => tree_sitter_jinja2::LANGUAGE.into(),
        "prisma" => tree_sitter_prisma::LANGUAGE.into(),
        "properties" => tree_sitter_properties::LANGUAGE.into(),
        "toml" => tree_sitter_toml_ng::LANGUAGE.into(),
        _ => return None,
    };
    Some(language)
}

/// Number of statically linked grammars.
pub fn grammar_count() -> usize {
    GRAMMAR_NAMES.len()
}

pub const GRAMMAR_NAMES: &[&str] = &[
    "rust",
    "python",
    "javascript",
    "typescript",
    "tsx",
    "go",
    "c",
    "cpp",
    "java",
    "csharp",
    "ruby",
    "php",
    "kotlin",
    "swift",
    "scala",
    "lua",
    "bash",
    "elixir",
    "haskell",
    "ocaml",
    "dart",
    "r",
    "julia",
    "zig",
    "nix",
    "hcl",
    "yaml",
    "json",
    "markdown",
    "dockerfile",
    "make",
    "cmake",
    "sql",
    "graphql",
    "html",
    "css",
    "scss",
    "vue",
    "svelte",
    "groovy",
    "erlang",
    "fsharp",
    "solidity",
    "powershell",
    "pascal",
    "d",
    "verilog",
    "vhdl",
    "ada",
    "commonlisp",
    "scheme",
    "elm",
    "fortran",
    "cuda",
    "glsl",
    "starlark",
    "ini",
    "diff",
    "regex",
    "gdscript",
    "luau",
    "nickel",
    "crystal",
    "qmljs",
    "jinja2",
    "prisma",
    "properties",
    "toml",
];

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    #[test]
    fn every_named_grammar_compiles_a_parser() {
        for name in GRAMMAR_NAMES {
            let language = grammar(name).unwrap_or_else(|| panic!("missing grammar {name}"));
            let mut parser = Parser::new();
            parser
                .set_language(&language)
                .unwrap_or_else(|e| panic!("grammar {name} incompatible: {e}"));
        }
    }

    #[test]
    fn parses_smoke_snippets() {
        let cases = [
            ("rust", "fn main() {}"),
            ("python", "def f(x):\n    return x\n"),
            ("javascript", "function f(x){return x}"),
            ("typescript", "function f(x: number): number { return x }"),
            ("go", "package main\nfunc main() {}\n"),
            ("c", "int main(void){return 0;}\n"),
            ("cpp", "class A { public: void f(); };\n"),
            ("java", "class A { void f() {} }\n"),
            ("csharp", "class A { void F() {} }\n"),
            ("ruby", "def f(x)\n x\nend\n"),
            ("php", "<?php function f($x) { return $x; }"),
            ("kotlin", "fun f(x: Int): Int { return x }"),
            ("swift", "func f(x: Int) -> Int { return x }"),
            ("markdown", "# Title\n\nText\n"),
            ("yaml", "a: 1\n"),
            ("toml", "a = 1\n"),
            ("dockerfile", "FROM alpine\n"),
        ];
        for (name, source) in cases {
            let lang = grammar(name).unwrap();
            let mut parser = Parser::new();
            parser.set_language(&lang).unwrap();
            let tree = parser.parse(source, None).unwrap();
            assert!(!tree.root_node().has_error(), "grammar {name} errored");
        }
    }
}
