//! Documentation ingestion: Markdown, reStructuredText, AsciiDoc, PDF text,
//! ADR metadata, diagrams and OpenAPI operations.

use crate::model::*;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

static MD_HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*#*\s*$").unwrap());
static MD_LINK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#).unwrap());
static MD_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`\n]+)`").unwrap());
static RST_UNDERLINE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^([=\-~^"'`#*+:.]+)\s*$"#).unwrap());
static ADR_ID: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(ADR|DEC|RFC|MADR)[-_ ]?0*(\d{1,5})\b").unwrap());
static SUPERSEDES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:supersedes|replaces)\b[:\s]+([^\n]{1,200})").unwrap());
static SUPERSEDED_BY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bsuperseded\s+by\b[:\s]+([^\n]{1,200})").unwrap());
static FIELD_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*[-*]?\s*(status|date|deciders?|decision-makers?)\s*[:\-]\s*(.+?)\s*$")
        .unwrap()
});
static PDF_TJ_STRING: Lazy<Regex> = Lazy::new(|| Regex::new(r"\((?:\\.|[^\\)])*\)\s*Tj").unwrap());
static PDF_TJ_ARRAY: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[(.*?)\]\s*TJ").unwrap());
static PDF_STRING: Lazy<Regex> = Lazy::new(|| Regex::new(r"\((?:\\.|[^\\)])*\)").unwrap());

pub fn parse_markdown(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<&str> = text.lines().collect();
    parse_markdown_sections(rel_path, &lines, out);
    parse_adr(rel_path, &text, out);
    parse_diagrams(rel_path, &lines, out);
}

fn parse_markdown_sections(_rel_path: &str, lines: &[&str], out: &mut FileExtract) {
    let mut headings: Vec<(u32, String, usize)> = Vec::new();
    let mut in_fence = false;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(cap) = MD_HEADING.captures(line) {
            let level = cap.get(1).unwrap().as_str().len() as u32;
            let title = cap.get(2).unwrap().as_str().trim().to_string();
            headings.push((level, title, idx));
        }
    }
    if headings.is_empty() {
        out.file_docstring = Some(text_without_fences(lines).trim().to_string());
        return;
    }
    if out.file_docstring.is_none() {
        out.file_docstring = Some(
            headings
                .iter()
                .map(|(_, t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    for (i, (level, title, start)) in headings.iter().enumerate() {
        let end = headings
            .iter()
            .skip(i + 1)
            .find(|(next_level, _, _)| next_level <= level)
            .map(|(_, _, idx)| *idx)
            .unwrap_or(lines.len());
        let body = lines[start + 1..end].join("\n");
        out.sections.push(DocSection {
            level: *level,
            title: title.clone(),
            start_line: *start as i32 + 1,
            end_line: end as i32,
            body: body.chars().take(20_000).collect(),
            anchor: Some(slugify(title)),
        });
    }
    for (idx, line) in lines.iter().enumerate() {
        for cap in MD_LINK.captures_iter(line) {
            let target = cap.get(2).unwrap().as_str();
            let kind = if target.starts_with("http://") || target.starts_with("https://") {
                "link"
            } else if looks_like_doc_or_file(target) {
                "file"
            } else {
                "link"
            };
            if kind == "file" {
                out.file_refs.push(FileRef {
                    path: target.trim_start_matches("./").to_string(),
                    line: idx as i32 + 1,
                });
            }
            out.doc_refs.push(DocRef {
                raw: target.to_string(),
                line: idx as i32 + 1,
                syntax: "markdown_link".to_string(),
                target_kind: kind.to_string(),
            });
        }
        for cap in MD_CODE.captures_iter(line) {
            let raw = cap.get(1).unwrap().as_str().trim();
            if looks_like_symbol(raw) {
                out.doc_refs.push(DocRef {
                    raw: raw.to_string(),
                    line: idx as i32 + 1,
                    syntax: "inline_code".to_string(),
                    target_kind: "code".to_string(),
                });
            }
        }
    }
}

pub fn parse_rst(_rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<&str> = text.lines().collect();
    let mut headings: Vec<(u32, String, usize)> = Vec::new();
    for idx in 0..lines.len().saturating_sub(1) {
        let title = lines[idx].trim();
        if title.is_empty() {
            continue;
        }
        let underline = lines[idx + 1].trim();
        if let Some(cap) = RST_UNDERLINE.captures(underline) {
            let ch = cap.get(1).unwrap().as_str().chars().next().unwrap_or('=');
            if underline.len() >= title.len().min(3) {
                let level = match ch {
                    '=' => 1,
                    '-' => 2,
                    '~' => 3,
                    '^' => 4,
                    _ => 5,
                };
                headings.push((level, title.to_string(), idx));
            }
        }
    }
    for (i, (level, title, start)) in headings.iter().enumerate() {
        let end = headings
            .iter()
            .skip(i + 1)
            .find(|(l, _, _)| l <= level)
            .map(|(_, _, idx)| *idx)
            .unwrap_or(lines.len());
        let body = lines[(start + 2).min(lines.len())..end].join("\n");
        out.sections.push(DocSection {
            level: *level,
            title: title.clone(),
            start_line: *start as i32 + 1,
            end_line: end as i32,
            body: body.chars().take(20_000).collect(),
            anchor: Some(slugify(title)),
        });
    }
    if out.file_docstring.is_none() {
        out.file_docstring = headings
            .first()
            .map(|(_, t, _)| t.clone())
            .or_else(|| Some(text.lines().take(5).collect::<Vec<_>>().join("\n")));
    }
    for (idx, line) in lines.iter().enumerate() {
        for cap in MD_LINK.captures_iter(line) {
            let target = cap.get(2).unwrap().as_str();
            if looks_like_doc_or_file(target) {
                out.file_refs.push(FileRef {
                    path: target.to_string(),
                    line: idx as i32 + 1,
                });
            }
        }
        // `text <path>`_ style links.
        if let Some(start) = line.find('<') {
            if let Some(end) = line[start..].find('>') {
                let target = &line[start + 1..start + end];
                if looks_like_doc_or_file(target) {
                    out.file_refs.push(FileRef {
                        path: target.to_string(),
                        line: idx as i32 + 1,
                    });
                }
            }
        }
        for cap in MD_CODE.captures_iter(line) {
            let raw = cap.get(1).unwrap().as_str().trim();
            if looks_like_symbol(raw) {
                out.doc_refs.push(DocRef {
                    raw: raw.to_string(),
                    line: idx as i32 + 1,
                    syntax: "rst_literal".to_string(),
                    target_kind: "code".to_string(),
                });
            }
        }
    }
}

pub fn parse_asciidoc(_rel_path: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<&str> = text.lines().collect();
    let mut headings: Vec<(u32, String, usize)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('=') {
            let level = rest.chars().take_while(|c| *c == '=').count() as u32 + 1;
            let title = rest.trim_start_matches('=').trim().to_string();
            if !title.is_empty() {
                headings.push((level, title, idx));
            }
        }
    }
    for (i, (level, title, start)) in headings.iter().enumerate() {
        let end = headings
            .iter()
            .skip(i + 1)
            .find(|(l, _, _)| l <= level)
            .map(|(_, _, idx)| *idx)
            .unwrap_or(lines.len());
        out.sections.push(DocSection {
            level: *level,
            title: title.clone(),
            start_line: *start as i32 + 1,
            end_line: end as i32,
            body: lines[start + 1..end]
                .join("\n")
                .chars()
                .take(20_000)
                .collect(),
            anchor: Some(slugify(title)),
        });
    }
    if out.file_docstring.is_none() {
        out.file_docstring = headings.first().map(|(_, t, _)| t.clone());
    }
    for (idx, line) in lines.iter().enumerate() {
        if let Some(rest) = line.trim_start().strip_prefix("include::") {
            let target = rest.trim_end_matches('[').trim_end_matches(']').trim();
            if !target.is_empty() {
                out.file_refs.push(FileRef {
                    path: target.to_string(),
                    line: idx as i32 + 1,
                });
            }
        }
        if let Some(rest) = line.trim_start().strip_prefix("xref:") {
            let target = rest.split('[').next().unwrap_or("");
            if !target.is_empty() {
                out.doc_refs.push(DocRef {
                    raw: target.to_string(),
                    line: idx as i32 + 1,
                    syntax: "asciidoc_xref".to_string(),
                    target_kind: "code".to_string(),
                });
            }
        }
    }
    parse_diagrams("", &lines, out);
}

/// Extract text from PDF content streams.
///
/// Only text layers are supported (no OCR). Compressed
/// streams are not decoded by this minimal reader; when no text is found the
/// file is reported as a coverage gap instead of pretending it was indexed.
pub fn parse_pdf(rel_path: &str, source: &[u8], out: &mut FileExtract) {
    if !source.starts_with(b"%PDF") {
        out.parse_gaps.push(ParseGap {
            start_line: 1,
            end_line: 1,
            reason: "pdf_invalid_header".to_string(),
        });
        return;
    }
    let extracted = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(source))
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if !extracted.trim().is_empty() {
        let mut line = 1i32;
        for paragraph in extracted.split(['\n', '\r']) {
            let paragraph = paragraph.trim();
            if paragraph.is_empty() {
                continue;
            }
            out.sections.push(DocSection {
                level: 1,
                title: format!("Page text {line}"),
                start_line: line,
                end_line: line,
                body: paragraph.chars().take(20_000).collect(),
                anchor: None,
            });
            line += 1;
        }
        out.file_docstring = Some(extracted.chars().take(4000).collect());
        return;
    }
    let lossy = String::from_utf8_lossy(source);
    let mut chunks = Vec::new();
    for cap in PDF_TJ_STRING.captures_iter(&lossy) {
        if let Some(m) = cap.get(0) {
            let raw = m.as_str();
            if let Some(start) = raw.find('(') {
                if let Some(end) = raw.rfind(')') {
                    chunks.push(unescape_pdf(&raw[start + 1..end]));
                }
            }
        }
    }
    for cap in PDF_TJ_ARRAY.captures_iter(&lossy) {
        if let Some(inner) = cap.get(1) {
            for part in PDF_STRING.captures_iter(inner.as_str()) {
                if let Some(m) = part.get(0) {
                    chunks.push(unescape_pdf(&m.as_str()[1..m.as_str().len() - 1]));
                }
            }
        }
    }
    let text = chunks.join(" ");
    if text.trim().is_empty() {
        out.parse_gaps.push(ParseGap {
            start_line: 1,
            end_line: out.lines.max(1) as i32,
            reason: "pdf_no_text_layer".to_string(),
        });
        return;
    }
    let mut line = 1i32;
    for paragraph in text.split(['\n', '\r']) {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        out.sections.push(DocSection {
            level: 1,
            title: format!("Page text {line}"),
            start_line: line,
            end_line: line,
            body: paragraph.chars().take(20_000).collect(),
            anchor: None,
        });
        line += 1;
    }
    out.file_docstring = Some(text.chars().take(4000).collect());
    let _ = rel_path;
}

fn unescape_pdf(raw: &str) -> String {
    raw.replace("\\(", "(")
        .replace("\\)", ")")
        .replace("\\\\", "\\")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
}

fn text_without_fences(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut fence = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if !fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn parse_openapi(_rel_path: &str, lang: &str, source: &[u8], out: &mut FileExtract) {
    if !matches!(lang, "yaml" | "json" | "openapi") {
        return;
    }
    let text = String::from_utf8_lossy(source);
    if !(text.contains("openapi:") || text.contains("\"openapi\"") || text.contains("swagger:")) {
        return;
    }
    let value: Value = if lang == "json" {
        match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return,
        }
    } else {
        match serde_yaml::from_str::<Value>(&text) {
            Ok(v) => v,
            Err(_) => return,
        }
    };
    let Some(paths) = value.get("paths").and_then(Value::as_object) else {
        return;
    };
    for (path, item) in paths {
        let Some(methods) = item.as_object() else {
            continue;
        };
        for (method, op) in methods {
            let method_upper = method.to_ascii_uppercase();
            if !matches!(
                method_upper.as_str(),
                "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS" | "TRACE"
            ) {
                continue;
            }
            let operation_id = op
                .get("operationId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let summary = op
                .get("summary")
                .or_else(|| op.get("description"))
                .and_then(Value::as_str)
                .map(|s| s.chars().take(300).collect());
            let line = text.lines().position(|l| l.contains(path)).unwrap_or(0) as i32 + 1;
            out.openapi.push(OpenApiOperation {
                method: method_upper,
                path: path.clone(),
                operation_id,
                summary,
                line,
            });
        }
    }
}

fn parse_adr(rel_path: &str, text: &str, out: &mut FileExtract) {
    let path_lower = rel_path.to_ascii_lowercase();
    let in_adr_dir = path_lower.split('/').any(|seg| {
        matches!(
            seg,
            "adr" | "adrs" | "decisions" | "decision-records" | "decisions-records"
        )
    });
    let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let id_cap = ADR_ID.captures(basename).or_else(|| ADR_ID.captures(text));
    let has_structure = text.to_ascii_lowercase().contains("status")
        && (text.to_ascii_lowercase().contains("context")
            || text.to_ascii_lowercase().contains("decision")
            || text.to_ascii_lowercase().contains("consequences"));
    if !(in_adr_dir && (id_cap.is_some() || has_structure)) && !(id_cap.is_some() && has_structure)
    {
        return;
    }
    let (id, title) = if let Some(cap) = id_cap {
        let prefix = cap.get(1).unwrap().as_str().to_ascii_uppercase();
        let number: u32 = cap.get(2).unwrap().as_str().parse().unwrap_or(0);
        let title = text
            .lines()
            .find(|l| l.trim_start().starts_with("# "))
            .map(|l| l.trim_start_matches('#').trim().to_string())
            .unwrap_or_else(|| basename.to_string());
        (format!("{prefix}-{number}"), title)
    } else {
        (
            basename
                .trim_end_matches(".md")
                .trim_end_matches(".markdown")
                .to_string(),
            text.lines()
                .find(|l| l.trim_start().starts_with("# "))
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .unwrap_or_else(|| basename.to_string()),
        )
    };
    let mut status = None;
    let mut date = None;
    let mut deciders = Vec::new();
    for line in text.lines().take(120) {
        if let Some(cap) = FIELD_LINE.captures(line) {
            let key = cap.get(1).unwrap().as_str().to_ascii_lowercase();
            let value = cap.get(2).unwrap().as_str().trim().to_string();
            match key.as_str() {
                "status" => status = Some(value),
                "date" => date = Some(value),
                "decider" | "deciders" | "decision-maker" | "decision-makers" => {
                    deciders = value
                        .split([',', ';', '&'])
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                _ => {}
            }
        }
    }
    let supersedes = SUPERSEDES
        .captures_iter(text)
        .map(|c| c.get(1).unwrap().as_str().trim().to_string())
        .collect();
    let superseded_by = SUPERSEDED_BY
        .captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()));
    let docstring = text
        .lines()
        .skip_while(|l| l.trim().is_empty())
        .take(20)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(2000)
        .collect();
    out.adr = Some(AdrRecord {
        id,
        title,
        status,
        date,
        deciders,
        superseded_by,
        supersedes,
        docstring: Some(docstring),
    });
}

fn parse_diagrams(_rel_path: &str, lines: &[&str], out: &mut FileExtract) {
    let mut fence = false;
    let mut is_mermaid = false;
    let mut start = 0usize;
    let mut block: Vec<&str> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if !fence {
                fence = true;
                is_mermaid = trimmed
                    .trim_matches(['`', '~'])
                    .trim()
                    .eq_ignore_ascii_case("mermaid");
                start = idx;
                block.clear();
            } else {
                if is_mermaid {
                    if let Some(diagram) = parse_mermaid(&block, start as i32 + 1) {
                        out.diagrams.push(diagram);
                    }
                }
                fence = false;
                is_mermaid = false;
            }
            continue;
        }
        if fence && is_mermaid {
            block.push(line);
        }
    }
}

fn parse_mermaid(lines: &[&str], start_line: i32) -> Option<Diagram> {
    let kind = lines
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("flowchart")
        .split_whitespace()
        .next()
        .unwrap_or("flowchart")
        .to_string();
    let mut elements = Vec::new();
    let mut arrows = Vec::new();
    let node_re =
        Regex::new(r"([A-Za-z0-9_]+)\s*(?:\[([^\]]*)\]|\(([^)]*)\)|\{([^}]*)\})").unwrap();
    let arrow_re = Regex::new(
        r"([A-Za-z0-9_]+(?:\[[^\]]*\]|\([^)]*\)|\{[^}]*\})?)\s*(?:-->|---|===>|-.->)\s*(?:\|([^|]*)\|\s*)?([A-Za-z0-9_]+(?:\[[^\]]*\]|\([^)]*\)|\{[^}]*\})?)",
    )
    .unwrap();
    for (idx, line) in lines.iter().enumerate() {
        for cap in node_re.captures_iter(line) {
            let id = cap.get(1).unwrap().as_str().to_string();
            let label = cap
                .get(2)
                .or_else(|| cap.get(3))
                .or_else(|| cap.get(4))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| id.clone());
            if !elements.iter().any(|e: &DiagramElement| e.id == id) {
                elements.push(DiagramElement {
                    id,
                    label,
                    line: start_line + idx as i32,
                });
            }
        }
        if let Some(cap) = arrow_re.captures(line) {
            let source = cap.get(1).unwrap().as_str();
            let target = cap.get(3).unwrap().as_str();
            arrows.push(DiagramArrow {
                source: clean_mermaid_id(source),
                target: clean_mermaid_id(target),
                line: start_line + idx as i32,
                label: cap.get(2).map(|m| m.as_str().to_string()),
            });
        }
    }
    if elements.is_empty() && arrows.is_empty() {
        return None;
    }
    Some(Diagram {
        name: format!("diagram-{start_line}"),
        line: start_line,
        kind,
        elements,
        arrows,
    })
}

fn clean_mermaid_id(raw: &str) -> String {
    raw.split(['[', '(', '{'])
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string()
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

fn looks_like_symbol(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() < 200
        && raw
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && raw
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '#' | '(' | ')' | '*'))
}

fn looks_like_doc_or_file(raw: &str) -> bool {
    if raw.starts_with('#') || raw.is_empty() {
        return false;
    }
    let raw = raw.split('#').next().unwrap_or(raw);
    let ext = raw.rsplit('.').next().unwrap_or("");
    !ext.is_empty()
        && ext.len() <= 8
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && !raw.starts_with("http")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, src: &str) -> FileExtract {
        let lang = gry_core::language::detect(name).unwrap();
        crate::extract::extract_file(name, lang, src.as_bytes())
    }

    #[test]
    fn markdown_sections_and_links() {
        let src = "# Guide\n\nIntro `Engine`.\n\n## Setup\n\nSee [parser](src/parser.rs).\n\n### Detail\n\nText.\n";
        let out = doc("docs/guide.md", src);
        assert_eq!(out.sections.len(), 3);
        assert_eq!(out.sections[0].title, "Guide");
        assert_eq!(out.sections[1].title, "Setup");
        assert!(out.sections[1].body.contains("parser"));
        assert!(out.file_refs.iter().any(|f| f.path == "src/parser.rs"));
        assert!(out.doc_refs.iter().any(|d| d.raw == "Engine"));
    }

    #[test]
    fn markdown_mermaid_diagram() {
        let src = "# Arch\n\n```mermaid\nflowchart LR\n  A[Client] --> B[API]\n```\n";
        let out = doc("docs/arch.md", src);
        assert_eq!(out.diagrams.len(), 1);
        assert_eq!(out.diagrams[0].elements.len(), 2);
        assert_eq!(out.diagrams[0].arrows.len(), 1);
    }

    #[test]
    fn adr_detection() {
        let src = "# ADR-12 Use SQLite\n\nStatus: Accepted\nDate: 2026-01-02\nDeciders: Ada, Grace\n\n## Context\n\nNeed storage.\n\n## Decision\n\nUse SQLite.\n\n## Consequences\n\nGood.\n\n## Supersedes\n\nADR-3\n";
        let out = doc("docs/adr/0012-use-sqlite.md", src);
        let adr = out.adr.expect("adr");
        assert_eq!(adr.id, "ADR-12");
        assert_eq!(adr.status.as_deref(), Some("Accepted"));
        assert_eq!(adr.deciders, vec!["Ada", "Grace"]);
        assert!(adr.supersedes.iter().any(|s| s.contains("ADR-3")));
    }

    #[test]
    fn pdf_text_layer_is_extracted() {
        let bytes = include_bytes!("../tests/fixtures/sample.pdf");
        let mut out = FileExtract::default();
        parse_pdf("docs/sample.pdf", bytes, &mut out);
        assert!(!out.sections.is_empty(), "expected a text layer section");
        assert!(out.file_docstring.is_some());
    }

    #[test]
    fn openapi_operations() {
        let src = "openapi: 3.0.0\npaths:\n  /users:\n    get:\n      operationId: listUsers\n      summary: List users\n    post:\n      operationId: createUser\n";
        let out = doc("api/openapi.yaml", src);
        assert_eq!(out.openapi.len(), 2);
        assert!(out
            .openapi
            .iter()
            .any(|o| o.method == "GET" && o.operation_id.as_deref() == Some("listUsers")));
    }
}
