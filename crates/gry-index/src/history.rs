//! Bounded Git-history change coupling (FILE_CHANGES_WITH).

use crate::graph::GraphBuilder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::process::Command;

/// Add FILE_CHANGES_WITH edges for files that change together in recent
/// history. Bounded to 2000 commits and 20 files/commit to keep work predictable.
pub fn add_file_coupling(builder: &mut GraphBuilder) {
    let root = builder.root_path.clone();
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args([
            "log",
            "--since=6 months ago",
            "--name-only",
            "--pretty=format:__COMMIT__%H",
            "-n",
            "2000",
        ])
        .output();
    let Ok(output) = output else { return };
    if !output.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pair_counts: HashMap<(String, String), i64> = HashMap::new();
    let mut file_commits: HashMap<String, i64> = HashMap::new();
    let mut current: HashSet<String> = HashSet::new();
    let mut commits = 0i64;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("__COMMIT__") {
            if !current.is_empty() {
                commits += 1;
                let files: Vec<String> = current.iter().cloned().collect();
                if files.len() <= 20 {
                    for f in &files {
                        *file_commits.entry(f.clone()).or_insert(0) += 1;
                    }
                    for i in 0..files.len() {
                        for j in i + 1..files.len() {
                            let key = if files[i] < files[j] {
                                (files[i].clone(), files[j].clone())
                            } else {
                                (files[j].clone(), files[i].clone())
                            };
                            *pair_counts.entry(key).or_insert(0) += 1;
                        }
                    }
                }
                current.clear();
            }
            continue;
        }
        current.insert(line.to_string());
    }
    if !current.is_empty() && current.len() <= 20 {
        commits += 1;
        let files: Vec<String> = current.into_iter().collect();
        for f in &files {
            *file_commits.entry(f.clone()).or_insert(0) += 1;
        }
        for i in 0..files.len() {
            for j in i + 1..files.len() {
                let key = if files[i] < files[j] {
                    (files[i].clone(), files[j].clone())
                } else {
                    (files[j].clone(), files[i].clone())
                };
                *pair_counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    if commits == 0 {
        return;
    }
    let mut pairs: Vec<((String, String), i64)> = pair_counts
        .into_iter()
        .filter(|((_, _), count)| *count >= 3)
        .collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for ((a, b), count) in pairs.into_iter().take(5000) {
        let (Some(qa), Some(qb)) = (
            builder.file_qn.get(&a).cloned(),
            builder.file_qn.get(&b).cloned(),
        ) else {
            continue;
        };
        let max_changes = file_commits
            .get(&a)
            .copied()
            .unwrap_or(1)
            .max(file_commits.get(&b).copied().unwrap_or(1))
            .max(1);
        let score = (count as f64 / max_changes as f64 * 1000.0).round() / 1000.0;
        let mut props = serde_json::Map::new();
        props.insert("count".into(), Value::Number(count.into()));
        props.insert(
            "score".into(),
            Value::Number(serde_json::Number::from_f64(score).unwrap_or(0.into())),
        );
        builder.add_edge(&qa, &qb, "FILE_CHANGES_WITH", props.clone());
        builder.add_edge(&qb, &qa, "FILE_CHANGES_WITH", props);
    }
}
