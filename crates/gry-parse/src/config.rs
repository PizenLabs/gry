//! Configuration and package-manifest parsing.
//!
//! Config key values are never stored: only key paths and source lines, so
//! secrets in `.env`, Kubernetes manifests or application config stay out of
//! the graph. Package manifests contribute dependency names and package
//! identity.

use crate::model::*;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;

static INI_SECTION: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\[([^\]]+)\]\s*$").unwrap());
static KEY_VALUE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*([A-Za-z0-9_.\-/]+)\s*[:=]\s*(.*)$").unwrap());
static GO_REQUIRE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*([A-Za-z0-9_.\-/]+)\s+v[0-9]").unwrap());
static GRADLE_DEP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:implementation|api|compile|testImplementation|runtimeOnly)\s*[\( ]\s*['"]([^'"]+)['"]"#)
        .unwrap()
});
static XML_ARTIFACT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<artifactId>\s*([^<\s]+)\s*</artifactId>").unwrap());
static MIX_DEP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{:([A-Za-z0-9_]+),").unwrap());
static GEMSPEC_DEP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"add_(?:runtime_|development_)?dependency\s*\(?\s*["']([^"']+)["']"#).unwrap()
});
static SWIFT_DEP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\.package\([^)]*url:\s*"([^"]+)""#).unwrap());

pub fn parse_config(rel_path: &str, lang: &str, source: &[u8], out: &mut FileExtract) {
    let text = String::from_utf8_lossy(source);
    let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let infra_like = text.contains("apiVersion:")
        || basename == "Chart.yaml"
        || basename.starts_with("docker-compose")
        || basename.starts_with("compose.")
        || text.contains("openapi:")
        || text.contains("\"openapi\"");
    match lang {
        "json" | "openapi" => {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if !infra_like {
                    collect_json_keys(&value, "", &text, out, 0);
                }
                parse_json_manifest(basename, &value, out);
            }
        }
        "yaml" => {
            if let Ok(value) = serde_yaml::from_str::<Value>(&text) {
                if !infra_like {
                    collect_json_keys(&value, "", &text, out, 0);
                }
                parse_yaml_manifest(basename, &value, out);
            }
        }
        "toml" => {
            if let Ok(value) = toml::from_str::<toml::Value>(&text) {
                let value = toml_to_json(value);
                if !infra_like {
                    collect_json_keys(&value, "", &text, out, 0);
                }
                parse_toml_manifest(basename, &value, out);
            }
        }
        "ini" | "properties" | "dotenv" | "requirements" | "gitignore" | "gitattributes" => {
            parse_flat_config(lang, &text, out);
            if lang == "requirements" {
                out.package = Some(PackageInfo {
                    name: basename.trim_end_matches(".txt").to_string(),
                    ecosystem: "pypi".to_string(),
                    dependencies: text
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .map(|l| {
                            l.split(['=', '<', '>', '~', '!', '['])
                                .next()
                                .unwrap_or(l)
                                .trim()
                                .to_string()
                        })
                        .filter(|s| !s.is_empty())
                        .collect(),
                    version: None,
                });
            }
            parse_text_manifest(basename, &text, out);
        }
        _ => {
            parse_text_manifest(basename, &text, out);
        }
    }
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(k, v)| (k, toml_to_json(v)))
                .collect(),
        ),
    }
}

fn collect_json_keys(
    value: &Value,
    prefix: &str,
    source: &str,
    out: &mut FileExtract,
    depth: usize,
) {
    if depth > 12 || out.config_keys.len() >= 5000 {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                let line = find_line(source, key);
                out.config_keys.push(ConfigKey {
                    key: path.clone(),
                    line,
                    section: if prefix.is_empty() {
                        None
                    } else {
                        Some(prefix.to_string())
                    },
                    format: "structured".to_string(),
                });
                if child.is_object() || child.is_array() {
                    collect_json_keys(child, &path, source, out, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter().take(500) {
                if item.is_object() || item.is_array() {
                    collect_json_keys(item, prefix, source, out, depth + 1);
                }
            }
        }
        _ => {}
    }
}

fn parse_flat_config(lang: &str, text: &str, out: &mut FileExtract) {
    let mut section: Option<String> = None;
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if lang == "ini" {
            if let Some(cap) = INI_SECTION.captures(line) {
                section = Some(cap.get(1).unwrap().as_str().to_string());
                continue;
            }
        }
        if let Some(cap) = KEY_VALUE.captures(line) {
            let key = cap.get(1).unwrap().as_str().to_string();
            let full = match &section {
                Some(s) => format!("{s}.{key}"),
                None => key.clone(),
            };
            out.config_keys.push(ConfigKey {
                key: full,
                line: idx as i32 + 1,
                section: section.clone(),
                format: lang.to_string(),
            });
        }
    }
}

fn parse_json_manifest(basename: &str, value: &Value, out: &mut FileExtract) {
    if basename == "package.json" {
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(basename)
            .to_string();
        let mut deps = Vec::new();
        for section in [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            if let Some(map) = value.get(section).and_then(Value::as_object) {
                deps.extend(map.keys().cloned());
            }
        }
        deps.sort();
        deps.dedup();
        out.package = Some(PackageInfo {
            name,
            ecosystem: "npm".to_string(),
            dependencies: deps,
            version: value
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    } else if basename == "composer.json" {
        let mut deps = Vec::new();
        for section in ["require", "require-dev"] {
            if let Some(map) = value.get(section).and_then(Value::as_object) {
                deps.extend(map.keys().cloned());
            }
        }
        out.package = Some(PackageInfo {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(basename)
                .to_string(),
            ecosystem: "composer".to_string(),
            dependencies: deps,
            version: value
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
}

fn parse_yaml_manifest(basename: &str, value: &Value, out: &mut FileExtract) {
    if basename == "pubspec.yaml" {
        let mut deps = Vec::new();
        for section in ["dependencies", "dev_dependencies"] {
            if let Some(map) = value.get(section).and_then(Value::as_object) {
                deps.extend(map.keys().cloned());
            }
        }
        out.package = Some(PackageInfo {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("pubspec")
                .to_string(),
            ecosystem: "pub".to_string(),
            dependencies: deps,
            version: value
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
}

fn parse_toml_manifest(basename: &str, value: &Value, out: &mut FileExtract) {
    if basename == "Cargo.toml" {
        let package = value.get("package");
        let mut deps = Vec::new();
        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(map) = value.get(section).and_then(Value::as_object) {
                deps.extend(map.keys().cloned());
            }
        }
        out.package = Some(PackageInfo {
            name: package
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("crate")
                .to_string(),
            ecosystem: "cargo".to_string(),
            dependencies: deps,
            version: package
                .and_then(|p| p.get("version"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    } else if basename == "pyproject.toml" {
        let project = value.get("project");
        let mut deps = project
            .and_then(|p| p.get("dependencies"))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(|s| {
                        s.split(['=', '<', '>', '~', '!', '['])
                            .next()
                            .unwrap_or(s)
                            .trim()
                            .to_string()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(poetry) = value
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dependencies"))
            .and_then(Value::as_object)
        {
            deps.extend(poetry.keys().filter(|k| k.as_str() != "python").cloned());
        }
        deps.sort();
        deps.dedup();
        out.package = Some(PackageInfo {
            name: project
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("python-project")
                .to_string(),
            ecosystem: "pypi".to_string(),
            dependencies: deps,
            version: project
                .and_then(|p| p.get("version"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
}

fn parse_text_manifest(basename: &str, text: &str, out: &mut FileExtract) {
    if basename == "go.mod" {
        let name = text
            .lines()
            .find(|l| l.trim_start().starts_with("module "))
            .map(|l| l.trim_start_matches("module ").trim().to_string())
            .unwrap_or_else(|| "go-module".to_string());
        let mut deps = Vec::new();
        let mut in_require = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("require (") {
                in_require = true;
                continue;
            }
            if in_require && trimmed == ")" {
                in_require = false;
                continue;
            }
            if in_require || trimmed.starts_with("require ") {
                if let Some(cap) = GO_REQUIRE.captures(trimmed) {
                    deps.push(cap.get(1).unwrap().as_str().to_string());
                }
            }
        }
        deps.sort();
        deps.dedup();
        out.package = Some(PackageInfo {
            name,
            ecosystem: "go".to_string(),
            dependencies: deps,
            version: None,
        });
    } else if basename == "pom.xml" {
        let name = XML_ARTIFACT
            .captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "maven-project".to_string());
        let mut deps: Vec<String> = XML_ARTIFACT
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        deps.retain(|d| d != &name);
        deps.sort();
        deps.dedup();
        out.package = Some(PackageInfo {
            name,
            ecosystem: "maven".to_string(),
            dependencies: deps,
            version: None,
        });
    } else if matches!(basename, "build.gradle" | "build.gradle.kts") {
        let deps: HashSet<String> = GRADLE_DEP
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        let mut deps: Vec<String> = deps.into_iter().collect();
        deps.sort();
        out.package = Some(PackageInfo {
            name: "gradle-project".to_string(),
            ecosystem: "gradle".to_string(),
            dependencies: deps,
            version: None,
        });
    } else if basename == "mix.exs" {
        let deps: HashSet<String> = MIX_DEP
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        let mut deps: Vec<String> = deps.into_iter().collect();
        deps.sort();
        out.package = Some(PackageInfo {
            name: "mix-project".to_string(),
            ecosystem: "hex".to_string(),
            dependencies: deps,
            version: None,
        });
    } else if basename.ends_with(".gemspec") {
        let deps: HashSet<String> = GEMSPEC_DEP
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        let mut deps: Vec<String> = deps.into_iter().collect();
        deps.sort();
        out.package = Some(PackageInfo {
            name: basename.trim_end_matches(".gemspec").to_string(),
            ecosystem: "rubygems".to_string(),
            dependencies: deps,
            version: None,
        });
    } else if basename == "Package.swift" {
        let deps: HashSet<String> = SWIFT_DEP
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        let mut deps: Vec<String> = deps.into_iter().collect();
        deps.sort();
        out.package = Some(PackageInfo {
            name: "swift-package".to_string(),
            ecosystem: "swiftpm".to_string(),
            dependencies: deps,
            version: None,
        });
    }
}

fn find_line(source: &str, key: &str) -> i32 {
    let needle = format!("\"{key}\"");
    let needle2 = format!("{key}:");
    let needle3 = format!("{key} ");
    for (idx, line) in source.lines().enumerate().take(20_000) {
        if line.contains(&needle)
            || line.trim_start().starts_with(&needle2)
            || line.contains(&needle3)
        {
            return idx as i32 + 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str, src: &str) -> FileExtract {
        let lang = gry_core::language::detect(name).unwrap();
        crate::extract::extract_file(name, lang, src.as_bytes())
    }

    #[test]
    fn yaml_keys_are_collected_without_values() {
        let src = "server:\n  port: 8080\n  token: secret-value\n";
        let out = cfg("config/app.yaml", src);
        assert!(out.config_keys.iter().any(|k| k.key == "server.port"));
        assert!(out.config_keys.iter().any(|k| k.key == "server.token"));
        assert!(!format!("{:?}", out.config_keys).contains("secret-value"));
    }

    #[test]
    fn package_json_dependencies() {
        let src = r#"{"name":"web","version":"1.0.0","dependencies":{"react":"18","next":"14"},"devDependencies":{"vitest":"1"}}"#;
        let out = cfg("package.json", src);
        let pkg = out.package.unwrap();
        assert_eq!(pkg.name, "web");
        assert_eq!(pkg.ecosystem, "npm");
        assert!(pkg.dependencies.contains(&"react".to_string()));
        assert!(pkg.dependencies.contains(&"vitest".to_string()));
    }

    #[test]
    fn cargo_and_go_manifests() {
        let out = cfg(
            "Cargo.toml",
            "[package]\nname=\"gry\"\nversion=\"0.1.0\"\n[dependencies]\nserde=\"1\"\n",
        );
        let pkg = out.package.unwrap();
        assert_eq!(pkg.name, "gry");
        assert!(pkg.dependencies.contains(&"serde".to_string()));

        let out = cfg(
            "go.mod",
            "module example.com/api\n\ngo 1.22\n\nrequire (\n github.com/x/y v1.2.3\n)\n",
        );
        let pkg = out.package.unwrap();
        assert_eq!(pkg.name, "example.com/api");
        assert!(pkg
            .dependencies
            .iter()
            .any(|d| d.contains("github.com/x/y")));
    }

    #[test]
    fn dotenv_has_no_values() {
        let out = cfg(
            ".env",
            "DATABASE_URL=postgres://user:pass@host/db\nDEBUG=true\n",
        );
        assert_eq!(out.config_keys.len(), 2);
        assert!(!format!("{:?}", out.config_keys).contains("postgres"));
    }
}
