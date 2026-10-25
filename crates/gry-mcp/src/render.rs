//! Output rendering and deterministic response budgeting.

use serde_json::Value;

pub fn render(value: &Value, format: &str, max_tokens: usize) -> String {
    let limit = max_tokens.clamp(128, 1_000_000) * 4;
    let mut value = value.clone();
    let compact = format != "json";
    let mut text = if compact {
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    };
    if text.len() <= limit {
        return text;
    }
    shrink(&mut value, limit, compact);
    text = if compact {
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    };
    if text.len() <= limit {
        return text;
    }
    // Last resort: drop large string bodies and diagnostics.
    if let Value::Object(ref mut map) = value {
        for key in [
            "source",
            "docstring",
            "content",
            "coverage_rows",
            "properties",
        ] {
            if let Some(v) = map.get_mut(key) {
                if v.is_string() || v.is_array() || v.is_object() {
                    map.insert(format!("{key}_omitted"), Value::Bool(true));
                    map.remove(key);
                }
            }
        }
        map.insert("truncated".into(), Value::Bool(true));
    }
    text = if compact {
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    };
    if text.len() <= limit {
        return text;
    }
    let fallback = serde_json::json!({
        "truncated": true,
        "reason": "max_output_tokens too small for one coherent row",
        "hint": "Retry with a higher max_output_tokens.",
    });
    if compact {
        serde_json::to_string(&fallback).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string_pretty(&fallback).unwrap_or_else(|_| "{}".to_string())
    }
}

fn shrink(value: &mut Value, limit: usize, compact: bool) {
    let size = |v: &Value| {
        if compact {
            serde_json::to_string(v)
                .map(|s| s.len())
                .unwrap_or(usize::MAX)
        } else {
            serde_json::to_string_pretty(v)
                .map(|s| s.len())
                .unwrap_or(usize::MAX)
        }
    };
    if size(value) <= limit {
        return;
    }
    // Prioritised arrays: enrichment first, then lower-ranked rows.
    let paths: &[&[&str]] = &[
        &["semantic", "rows"],
        &["coverage_rows"],
        &["impact", "rows"],
        &["changed_files", "rows"],
        &["seeds", "rows"],
        &["modules", "rows"],
        &["results"],
        &["rows"],
    ];
    for path in paths {
        let arr_len = get_path(value, path)
            .and_then(Value::as_array)
            .map(|a| a.len());
        let Some(mut len) = arr_len else { continue };
        while len > 0 && size(value) > limit {
            len = len.saturating_sub(if len > 50 { 10 } else { 1 });
            if let Some(arr) = get_path_mut(value, path) {
                if let Value::Array(items) = arr {
                    items.truncate(len);
                }
            }
        }
        if let Some(parent) = get_parent_mut(value, path) {
            if let Value::Object(map) = parent {
                let key = path.last().unwrap();
                map.insert("truncated".into(), Value::Bool(true));
                map.insert("returned".into(), Value::Number(len.into()));
                map.insert("has_more".into(), Value::Bool(true));
                let _ = key;
            }
        }
        // Metadata added above counts toward the budget; trim more whole rows
        // if needed.
        while size(value) > limit && len > 0 {
            len -= 1;
            if let Some(arr) = get_path_mut(value, path) {
                if let Value::Array(items) = arr {
                    items.truncate(len);
                }
            }
            if let Some(parent) = get_parent_mut(value, path) {
                if let Value::Object(map) = parent {
                    map.insert("returned".into(), Value::Number(len.into()));
                }
            }
        }
        if size(value) <= limit {
            return;
        }
    }
    // Drop heavy diagnostic objects last.
    for key in ["coverage", "properties", "debug"] {
        if size(value) <= limit {
            return;
        }
        if let Value::Object(ref mut map) = value {
            if map.remove(key).is_some() {
                map.insert("diagnostics_omitted".into(), Value::Bool(true));
            }
        }
    }
}

fn get_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn get_path_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut current = value;
    for key in path {
        current = current.get_mut(*key)?;
    }
    Some(current)
}

fn get_parent_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut current = value;
    for key in &path[..path.len().saturating_sub(1)] {
        current = current.get_mut(*key)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn budget_drops_whole_rows_and_stays_valid_json() {
        let rows: Vec<Value> = (0..200)
            .map(|i| json!({"qn": format!("p.src.file{i}"), "name": format!("symbol_{i}"), "label": "Function"}))
            .collect();
        let value = json!({"total": 200, "returned": 200, "results": rows});
        let text = render(&value, "tree", 128);
        assert!(text.len() <= 128 * 4);
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert!(parsed["results"].as_array().unwrap().len() < 200);
        // Every surviving row is whole.
        for row in parsed["results"].as_array().unwrap() {
            assert!(row["qn"].is_string() && row["name"].is_string());
        }
    }

    #[test]
    fn json_format_is_pretty_and_under_budget() {
        let value = json!({"results": [{"qn":"a"},{"qn":"b"}], "total": 2});
        let text = render(&value, "json", 128);
        assert!(text.contains('\n'));
        assert!(text.len() <= 128 * 4);
    }

    #[test]
    fn tiny_budget_returns_actionable_fallback() {
        let value = json!({"results": (0..50).map(|i| json!({"qn": format!("a{i}"), "name": "x"})).collect::<Vec<_>>()});
        let text = render(&value, "tree", 128);
        assert!(serde_json::from_str::<Value>(&text).is_ok());
    }
}
