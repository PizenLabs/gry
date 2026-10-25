//! Architecture Decision Record management with byte-preserving section writes.

use gry_store::Store;
use serde_json::{json, Map, Value};

pub struct AdrRequest {
    pub project: String,
    pub mode: String,
    pub content: Option<String>,
    pub section_updates: Option<Map<String, Value>>,
    pub section_limit: usize,
    pub section_offset: usize,
}

pub fn manage_adr(store: &Store, req: &AdrRequest) -> gry_core::GryResult<Value> {
    store.require_project(&req.project)?;
    let mode = if req.mode.is_empty() {
        "outline"
    } else {
        req.mode.as_str()
    };
    match mode {
        "outline" => {
            let content = store.adr_get(&req.project)?.unwrap_or_default();
            let headings = headings(&content);
            let total = headings.len();
            let page: Vec<Value> = headings
                .into_iter()
                .skip(req.section_offset)
                .take(req.section_limit.max(1))
                .map(|h| json!({"name": h.name, "level": h.level, "line": h.line}))
                .collect();
            Ok(json!({
                "project": req.project,
                "mode": "outline",
                "present": !content.is_empty(),
                "bytes": content.len(),
                "lines": content.lines().count(),
                "headings_total": total,
                "headings": page,
                "has_more": req.section_offset + page.len() < total,
                "content": if content.is_empty() { Value::Null } else { Value::Null },
            }))
        }
        "get" => {
            let content = store.adr_get(&req.project)?.unwrap_or_default();
            Ok(json!({
                "project": req.project,
                "mode": "get",
                "present": !content.is_empty(),
                "content": content,
            }))
        }
        "sections" => {
            let content = store.adr_get(&req.project)?.unwrap_or_default();
            let rows: Vec<Value> = headings(&content)
                .into_iter()
                .map(|h| json!({"name": h.name, "level": h.level, "line": h.line}))
                .collect();
            Ok(json!({"project": req.project, "mode": "sections", "sections": rows}))
        }
        "update" | "store" => {
            let Some(content) = &req.content else {
                return Err(gry_core::GryError::invalid(
                    "content is required for update",
                ));
            };
            if content.contains('\0') {
                return Err(gry_core::GryError::invalid("content contains a NUL byte"));
            }
            store.adr_put(&req.project, content)?;
            Ok(json!({
                "project": req.project,
                "mode": "update",
                "status": "stored",
                "bytes": content.len(),
            }))
        }
        "set_sections" => {
            let Some(updates) = &req.section_updates else {
                return Err(gry_core::GryError::invalid(
                    "section_updates is required for set_sections",
                ));
            };
            let current = store.adr_get(&req.project)?.unwrap_or_default();
            let mut doc = current.clone();
            for (name, value) in updates {
                let Some(body) = value.as_str() else {
                    return Err(gry_core::GryError::invalid(format!(
                        "section body for {name} must be a string"
                    )));
                };
                validate_section_name(name)?;
                doc = splice_section(&doc, name, body);
            }
            if doc != current {
                store.adr_put(&req.project, &doc)?;
            }
            Ok(json!({
                "project": req.project,
                "mode": "set_sections",
                "status": if doc == current { "unchanged" } else { "stored" },
                "sections_written": updates.len(),
                "bytes": doc.len(),
            }))
        }
        other => Err(gry_core::GryError::invalid(format!(
            "invalid mode: {other}; use outline, get, sections, set_sections, or update"
        ))),
    }
}

#[derive(Debug, Clone)]
struct Heading {
    name: String,
    level: u32,
    line: usize,
    body_start: usize,
    body_end: usize,
}

fn headings(content: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let lines: Vec<&str> = content.lines().collect();
    let mut offsets = Vec::with_capacity(lines.len());
    let mut offset = 0usize;
    for line in &lines {
        offsets.push(offset);
        offset += line.len() + 1;
    }
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let Some((level, name)) = parse_heading(line) else {
            continue;
        };
        let body_start = offsets[idx] + line.len();
        // Section body ends at the next heading of any level (source ADR
        // section model).
        let body_end = lines
            .iter()
            .enumerate()
            .skip(idx + 1)
            .find(|(_, l)| parse_heading(l).is_some())
            .map(|(i, _)| offsets[i])
            .unwrap_or(content.len());
        out.push(Heading {
            name,
            level,
            line: idx + 1,
            body_start,
            body_end,
        });
    }
    out
}

fn parse_heading(line: &str) -> Option<(u32, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count() as u32;
    if level < 2 || level > 6 {
        return None;
    }
    let rest = trimmed[level as usize..].trim();
    if rest.is_empty() {
        None
    } else {
        Some((level, rest.to_string()))
    }
}

fn validate_section_name(name: &str) -> gry_core::GryResult<()> {
    if name.is_empty() || name.len() > 200 {
        return Err(gry_core::GryError::invalid(
            "invalid section name: must be 1..200 bytes",
        ));
    }
    if name.starts_with('#') || name.contains('\n') || name.contains('\r') {
        return Err(gry_core::GryError::invalid(format!(
            "invalid section name {name:?}: must not start with '#' or contain newlines"
        )));
    }
    if name.trim() != name {
        return Err(gry_core::GryError::invalid(format!(
            "invalid section name {name:?}: leading/trailing whitespace is not allowed"
        )));
    }
    Ok(())
}

fn splice_section(content: &str, name: &str, body: &str) -> String {
    let eol = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let headings = headings(content);
    let existing = headings.iter().find(|h| h.name == name);
    if let Some(heading) = existing {
        let old = &content[heading.body_start..heading.body_end];
        let old_trim = old.trim_matches(['\r', '\n', ' ', '\t']);
        let new_trim = body.trim_matches(['\r', '\n', ' ', '\t']);
        if old_trim == new_trim {
            return content.to_string();
        }
        // Preserve the blank line after the heading if present; replace the
        // body while leaving the heading and surrounding bytes intact.
        let mut replacement = String::new();
        replacement.push_str(eol);
        let body_trimmed = body.trim_end_matches(['\r', '\n']);
        replacement.push_str(body_trimmed);
        replacement.push_str(eol);
        if heading.body_end < content.len() {
            replacement.push_str(eol);
        }
        let mut out = String::with_capacity(content.len());
        out.push_str(&content[..heading.body_start]);
        out.push_str(&replacement);
        out.push_str(&content[heading.body_end..]);
        return out;
    }
    let mut out = content.to_string();
    if !out.is_empty() && !out.ends_with(['\n', '\r']) {
        out.push_str(eol);
    }
    if !out.is_empty() {
        out.push_str(eol);
    }
    out.push_str("## ");
    out.push_str(name);
    out.push_str(eol);
    out.push_str(eol);
    out.push_str(body.trim_end_matches(['\r', '\n']));
    out.push_str(eol);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    fn request(mode: &str) -> AdrRequest {
        AdrRequest {
            project: "p".into(),
            mode: mode.into(),
            content: None,
            section_updates: None,
            section_limit: 50,
            section_offset: 0,
        }
    }

    #[test]
    fn update_get_outline_and_sections() {
        let store = test_store();
        let mut req = request("update");
        req.content = Some("# ADR\n\n## PURPOSE\n\nWhy.\n\n## STACK\n\nRust.\n".into());
        assert_eq!(manage_adr(&store, &req).unwrap()["status"], "stored");
        let out = manage_adr(&store, &request("outline")).unwrap();
        assert_eq!(out["headings"].as_array().unwrap().len(), 2);
        assert_eq!(out["headings"][0]["name"], "PURPOSE");
        let out = manage_adr(&store, &request("get")).unwrap();
        assert!(out["content"].as_str().unwrap().contains("## STACK"));
    }

    #[test]
    fn set_sections_is_byte_preserving_and_idempotent() {
        let store = test_store();
        let mut req = request("update");
        req.content = Some("# ADR\n\nPreamble.\n\n## PURPOSE\n\nOld purpose.\n\n## STACK\n\nOld stack.\n\n```\ncode\n```\n".into());
        manage_adr(&store, &req).unwrap();

        let mut req = request("set_sections");
        let mut updates = Map::new();
        updates.insert("PURPOSE".into(), json!("New purpose."));
        req.section_updates = Some(updates);
        manage_adr(&store, &req).unwrap();
        let once = store.adr_get("p").unwrap().unwrap();
        assert!(once.contains("New purpose."));
        assert!(once.contains("Old stack."));
        assert!(once.contains("Preamble."));
        assert!(once.contains("code"));

        manage_adr(&store, &req).unwrap();
        let twice = store.adr_get("p").unwrap().unwrap();
        assert_eq!(once, twice);

        let mut updates = Map::new();
        updates.insert("NEW SECTION".into(), json!("Added."));
        let mut req = request("set_sections");
        req.section_updates = Some(updates);
        manage_adr(&store, &req).unwrap();
        let doc = store.adr_get("p").unwrap().unwrap();
        assert!(doc.contains("## NEW SECTION"));
        assert!(doc.contains("Added."));
    }

    #[test]
    fn invalid_names_are_rejected() {
        let store = test_store();
        let mut req = request("set_sections");
        let mut updates = Map::new();
        updates.insert("#bad".into(), json!("x"));
        req.section_updates = Some(updates);
        assert!(manage_adr(&store, &req).is_err());
    }
}
