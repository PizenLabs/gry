//! Lexical fallback for mapped languages without a compatible tree-sitter
//! grammar. It is deliberately conservative: only unambiguous declaration
//! forms become graph symbols, and relationships are not inferred.

use crate::model::{Def, FileExtract};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};

static PERL_SUB: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap());
static PERL_PACKAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)").unwrap());
static VIM_FUNC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*fu(?:nction)?!?\s+([A-Za-z_][A-Za-z0-9_:#.]*)").unwrap());
static FISH_FUNC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*function\s+([A-Za-z_][A-Za-z0-9_-]*)").unwrap());
static COBOL_PROGRAM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*PROGRAM-ID\.\s+([A-Za-z0-9_-]+)").unwrap());
static COBOL_SECTION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*([A-Za-z0-9_-]+)\s+SECTION\.").unwrap());
static C_FUNC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*(?:[A-Za-z_][A-Za-z0-9_<>:*&\[\]]*\s+)+([A-Za-z_][A-Za-z0-9_]*)\s*\([^;]*\)\s*\{?\s*$").unwrap()
});
static LISP_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*\((defun|defmacro|defclass|defstruct|defvar|defparameter)\s+([A-Za-z0-9_*+<>=!?/-]+)",
    )
    .unwrap()
});
static IDL_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*(service|struct|message|enum|interface|rpc|union|exception)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

pub fn extract(rel_path: &str, lang: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<&str> = text.lines().collect();
    match lang {
        "perl" => {
            for (idx, line) in lines.iter().enumerate() {
                if let Some(cap) = PERL_PACKAGE.captures(line) {
                    add(out, "Module", cap.get(1).unwrap().as_str(), idx + 1, None);
                } else if let Some(cap) = PERL_SUB.captures(line) {
                    let end = block_end(&lines, idx);
                    add(
                        out,
                        "Function",
                        cap.get(1).unwrap().as_str(),
                        idx + 1,
                        Some(end),
                    );
                }
            }
        }
        "vim" => {
            for (idx, line) in lines.iter().enumerate() {
                if let Some(cap) = VIM_FUNC.captures(line) {
                    let name = cap.get(1).unwrap().as_str();
                    let end = block_end(&lines, idx);
                    add(out, "Function", name, idx + 1, Some(end));
                }
            }
        }
        "fish" => {
            for (idx, line) in lines.iter().enumerate() {
                if let Some(cap) = FISH_FUNC.captures(line) {
                    let end = block_end(&lines, idx);
                    add(
                        out,
                        "Function",
                        cap.get(1).unwrap().as_str(),
                        idx + 1,
                        Some(end),
                    );
                }
            }
        }
        "cobol" => {
            for (idx, line) in lines.iter().enumerate() {
                if let Some(cap) = COBOL_PROGRAM.captures(line) {
                    add(out, "Module", cap.get(1).unwrap().as_str(), idx + 1, None);
                } else if let Some(cap) = COBOL_SECTION.captures(line) {
                    add(out, "Function", cap.get(1).unwrap().as_str(), idx + 1, None);
                }
            }
        }
        "glsl" | "hlsl" | "wgsl" => {
            for (idx, line) in lines.iter().enumerate() {
                if line.contains(';') || !line.contains('(') {
                    continue;
                }
                if let Some(cap) = C_FUNC.captures(line) {
                    let name = cap.get(1).unwrap().as_str();
                    if !matches!(name, "if" | "for" | "while" | "switch" | "return") {
                        let end = block_end(&lines, idx);
                        add(out, "Function", name, idx + 1, Some(end));
                    }
                }
            }
        }
        "commonlisp" | "scheme" | "elisp" => {
            for (idx, line) in lines.iter().enumerate() {
                if let Some(cap) = LISP_DEF.captures(line) {
                    let kind = cap.get(1).unwrap().as_str();
                    let label = match kind {
                        "defclass" | "defstruct" => "Class",
                        "defvar" | "defparameter" => "Variable",
                        "defmacro" => "Macro",
                        _ => "Function",
                    };
                    add(out, label, cap.get(2).unwrap().as_str(), idx + 1, None);
                }
            }
        }
        "thrift" | "capnp" | "protobuf" | "graphql" | "prisma" => {
            for (idx, line) in lines.iter().enumerate() {
                if let Some(cap) = IDL_DEF.captures(line) {
                    let kind = cap.get(1).unwrap().as_str();
                    let label = match kind {
                        "struct" | "union" | "message" => "Struct",
                        "enum" => "Enum",
                        "service" | "interface" => "Interface",
                        "rpc" => "Function",
                        _ => "Type",
                    };
                    add(out, label, cap.get(2).unwrap().as_str(), idx + 1, None);
                }
            }
        }
        _ => {}
    }
    let _ = rel_path;
}

fn add(out: &mut FileExtract, label: &str, name: &str, line: usize, end: Option<usize>) {
    if name.is_empty() {
        return;
    }
    let mut properties = Map::new();
    properties.insert("lexical_fallback".to_string(), Value::Bool(true));
    out.defs.push(Def {
        label: label.to_string(),
        name: name.to_string(),
        scope: Vec::new(),
        start_line: line as i32,
        end_line: end.unwrap_or(line) as i32,
        properties,
        ..Default::default()
    });
}

fn block_end(lines: &[&str], start: usize) -> usize {
    // Brace-balanced or indentation-based block end, bounded to 500 lines.
    let mut depth = 0i32;
    let mut started = false;
    let start_indent = lines[start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();
    for (offset, line) in lines[start..].iter().enumerate().take(500) {
        if offset > 0
            && !started
            && !line.trim().is_empty()
            && line.chars().take_while(|c| c.is_whitespace()).count() <= start_indent
        {
            return start + offset;
        }
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
                started = true;
            } else if ch == '}' {
                depth -= 1;
                if started && depth <= 0 {
                    return start + offset + 1;
                }
            }
        }
        if started && depth == 0 && line.contains('}') {
            return start + offset + 1;
        }
    }
    start + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perl_functions_are_extracted() {
        let mut out = FileExtract::default();
        extract(
            "a.pl",
            "perl",
            b"package Foo;\nsub bar {\n  return 1;\n}\n",
            &mut out,
        );
        assert!(out
            .defs
            .iter()
            .any(|d| d.name == "bar" && d.label == "Function"));
        assert!(out
            .defs
            .iter()
            .any(|d| d.name == "Foo" && d.label == "Module"));
    }

    #[test]
    fn lisp_definitions_are_extracted() {
        let mut out = FileExtract::default();
        extract(
            "a.lisp",
            "commonlisp",
            b"(defun hello (x) x)\n(defclass Thing () ())\n",
            &mut out,
        );
        assert!(out.defs.iter().any(|d| d.name == "hello"));
        assert!(out
            .defs
            .iter()
            .any(|d| d.name == "Thing" && d.label == "Class"));
    }
}
