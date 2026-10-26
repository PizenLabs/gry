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
    // Preserve scalar metadata and collapse the largest arrays to counts so
    // oversized reports (for example architecture overviews) stay actionable
    // instead of degrading to an empty error.
    let encoded = |v: &Value| {
        if compact {
            serde_json::to_string(v).unwrap_or_default()
        } else {
            serde_json::to_string_pretty(v).unwrap_or_default()
        }
    };
    let mut arrays: Vec<(String, usize, usize)> = match &value {
        Value::Object(map) => map
            .iter()
            .filter_map(|(key, item)| {
                item.as_array()
                    .filter(|items| !items.is_empty())
                    .map(|items| {
                        let bytes = serde_json::to_string(items)
                            .map(|text| text.len())
                            .unwrap_or(usize::MAX);
                        (key.clone(), bytes, items.len())
                    })
            })
            .collect(),
        _ => Vec::new(),
    };
    arrays.sort_by_key(|item| std::cmp::Reverse(item.1));
    for (key, _, count) in arrays {
        if encoded(&value).len() <= limit {
            break;
        }
        if let Value::Object(map) = &mut value {
            map.insert(key, serde_json::json!({"count": count, "omitted": true}));
        }
    }
    if let Value::Object(map) = &mut value {
        map.insert("truncated".into(), Value::Bool(true));
    }
    text = encoded(&value);
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

/// Keep paging metadata consistent after rows were removed. `has_more` must
/// never be true without a usable `next_offset`.
fn update_page(map: &mut serde_json::Map<String, Value>, returned: usize) {
    map.insert("returned".into(), Value::Number(returned.into()));
    let Some(total) = map.get("total").and_then(Value::as_u64) else {
        return;
    };
    let offset = map.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let has_more = offset + (returned as u64) < total;
    map.insert("has_more".into(), Value::Bool(has_more));
    map.insert(
        "next_offset".into(),
        if has_more {
            Value::Number((offset + (returned as u64)).into())
        } else {
            Value::Null
        },
    );
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
    // Trim supporting detail before the answer. Changed files and impact are
    // the reason the caller asked for detect_changes; seeds are context.
    let paths: &[&[&str]] = &[
        &["semantic", "rows"],
        &["coverage_rows"],
        &["seeds", "rows"],
        &["modules", "rows"],
        &["impact", "rows"],
        &["changed_files", "rows"],
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
            if let Some(Value::Array(items)) = get_path_mut(value, path) {
                items.truncate(len);
            }
        }
        if let Some(Value::Object(map)) = get_parent_mut(value, path) {
            map.insert("truncated".into(), Value::Bool(true));
            update_page(map, len);
        }
        // Metadata added above counts toward the budget; trim more whole rows
        // if needed.
        while size(value) > limit && len > 0 {
            len -= 1;
            if let Some(Value::Array(items)) = get_path_mut(value, path) {
                items.truncate(len);
            }
            if let Some(Value::Object(map)) = get_parent_mut(value, path) {
                update_page(map, len);
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

    #[test]
    fn truncation_prefers_changed_files_over_seeds() {
        let changed: Vec<Value> = (0..25)
            .map(|i| json!({"path": format!("f{i}.go"), "status": "A"}))
            .collect();
        let seeds: Vec<Value> = (0..400)
            .map(|i| json!({"qn": format!("seed{i}"), "name": format!("s{i}")}))
            .collect();
        let value = json!({
            "changed_files": {"total": 25, "offset": 0, "returned": 25, "has_more": false, "rows": changed},
            "impact": {"total": 1, "offset": 0, "returned": 1, "has_more": false, "rows": [{"qn": "x"}]},
            "seeds": {"total": 400, "offset": 0, "returned": 400, "has_more": false, "rows": seeds},
        });
        let text = render(&value, "tree", 512);
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["changed_files"]["returned"], 25);
        assert!(
            parsed["seeds"]["returned"].as_u64().unwrap() < 400,
            "seeds must be trimmed before changed files"
        );
    }

    #[test]
    fn truncation_sets_a_usable_next_offset() {
        let changed: Vec<Value> = (0..200)
            .map(|i| json!({"path": format!("f{i}.go"), "status": "A"}))
            .collect();
        let value = json!({
            "changed_files": {"total": 200, "offset": 0, "returned": 200, "has_more": false, "rows": changed},
            "seeds": {"total": 0, "offset": 0, "returned": 0, "has_more": false, "rows": []},
        });
        let text = render(&value, "tree", 256);
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let returned = parsed["changed_files"]["returned"].as_u64().unwrap();
        assert!(returned < 200);
        assert_eq!(parsed["changed_files"]["has_more"], true);
        assert_eq!(parsed["changed_files"]["next_offset"], returned);
    }

    #[test]
    fn oversized_architecture_keeps_metadata_and_collapses_arrays() {
        let packages: Vec<Value> = (0..400)
            .map(|i| json!({"name": format!("package_{i}"), "files": 1}))
            .collect();
        let value = json!({
            "project": "repo",
            "nodes": 1200,
            "languages": [{"language": "rust", "files": 10}],
            "packages": packages,
            "entry_points": [{"name": "main", "file": "src/main.rs"}],
        });
        let text = render(&value, "tree", 128);
        assert!(text.len() <= 128 * 4);
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["project"], "repo");
        assert_eq!(parsed["nodes"], 1200);
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["packages"]["count"], 400);
        assert_eq!(parsed["packages"]["omitted"], true);
    }
}
