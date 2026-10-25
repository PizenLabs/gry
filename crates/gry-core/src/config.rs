//! Runtime configuration and cache layout.
//!
//! Configuration sources:
//! - `GRY_CACHE_DIR` sets the cache root (default `~/.cache/gry`).
//! - `GRY_DB` selects the database file directly.
//! - `GRY_ALLOWED_ROOT` confines indexing.
//! - `GRY_WORKERS` bounds the worker pool.
//! - `GRY_MAX_INDEX_FILES`, `GRY_MAX_INDEX_SOURCE_MB` are per-run limits.

use crate::error::{GryError, GryResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSettings {
    #[serde(default)]
    pub auto_index: bool,
    #[serde(default = "default_auto_index_limit")]
    pub auto_index_limit: u64,
    #[serde(default = "default_true")]
    pub auto_watch: bool,
    /// `None` means "auto": on for full indexes, off for moderate/fast.
    #[serde(default)]
    pub doc_candidates: Option<bool>,
    #[serde(default = "default_true")]
    pub watcher_enabled: bool,
    #[serde(default)]
    pub watch_non_git: bool,
    #[serde(default)]
    pub index_max_files: Option<u64>,
    #[serde(default)]
    pub index_max_source_mb: Option<u64>,
    #[serde(default)]
    pub index_max_memory_mb: Option<u64>,
    #[serde(default = "default_true")]
    pub semantic_enabled: bool,
}

fn default_true() -> bool {
    true
}
fn default_auto_index_limit() -> u64 {
    50_000
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            auto_index: false,
            auto_index_limit: 50_000,
            auto_watch: true,
            doc_candidates: None,
            watcher_enabled: true,
            watch_non_git: false,
            index_max_files: None,
            index_max_source_mb: None,
            index_max_memory_mb: None,
            semantic_enabled: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub cache_dir: PathBuf,
    pub db_path: PathBuf,
    pub allowed_root: Option<PathBuf>,
    pub workers: usize,
    pub max_files: Option<u64>,
    pub max_source_mb: Option<u64>,
    pub settings: RuntimeSettings,
}

impl Config {
    pub fn load() -> GryResult<Self> {
        let cache_dir = resolve_cache_dir();
        let db_path = std::env::var_os("GRY_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache_dir.join("graph.db"));
        let allowed_root = std::env::var_os("GRY_ALLOWED_ROOT").map(PathBuf::from);
        let workers = std::env::var("GRY_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get().min(8))
                    .unwrap_or(4)
            });
        let max_files = std::env::var("GRY_MAX_INDEX_FILES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        let max_source_mb = std::env::var("GRY_MAX_INDEX_SOURCE_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        let settings = load_settings(&cache_dir);
        Ok(Self {
            cache_dir,
            db_path,
            allowed_root,
            workers,
            max_files: max_files.or(settings.index_max_files),
            max_source_mb: max_source_mb.or(settings.index_max_source_mb),
            settings,
        })
    }

    /// Reload settings after a `config set/reset` (used by long-lived servers).
    pub fn reload_settings(&mut self) {
        self.settings = load_settings(&self.cache_dir);
        self.max_files = self.settings.index_max_files;
        self.max_source_mb = self.settings.index_max_source_mb;
    }

    /// Soft index-memory budget in MiB. The environment wins over settings.
    pub fn max_index_memory_mb(&self) -> Option<u64> {
        std::env::var("GRY_MAX_INDEX_MEMORY_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(self.settings.index_max_memory_mb)
    }

    pub fn ensure_dirs(&self) -> GryResult<()> {
        fs::create_dir_all(&self.cache_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.cache_dir, fs::Permissions::from_mode(0o700));
        }
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Resolve and validate a repository path against the containment policy.
    pub fn resolve_index_root(&self, requested: &str) -> GryResult<PathBuf> {
        let path = PathBuf::from(requested);
        let canonical = path.canonicalize().map_err(|e| {
            GryError::invalid(format!(
                "repo_path does not exist or is not accessible: {requested}: {e}"
            ))
        })?;
        if !canonical.is_dir() {
            return Err(GryError::invalid(format!(
                "repo_path is not a directory: {}",
                canonical.display()
            )));
        }
        if let Some(root) = &self.allowed_root {
            let root = root.canonicalize().unwrap_or_else(|_| root.clone());
            if !canonical.starts_with(&root) {
                return Err(GryError::invalid(format!(
                    "repo_path resolves outside the allowed root policy: {} is not under {} \
                     (GRY_ALLOWED_ROOT)",
                    canonical.display(),
                    root.display()
                )));
            }
        }
        Ok(canonical)
    }

    pub fn settings_path(&self) -> PathBuf {
        self.cache_dir.join("settings.json")
    }

    pub fn save_settings(&self) -> GryResult<()> {
        self.ensure_dirs()?;
        let path = self.settings_path();
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&self.settings)?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn artifact_path(root: &Path) -> PathBuf {
        root.join(".gry").join("graph.db.zst")
    }

    pub fn job_dir(&self) -> PathBuf {
        self.cache_dir.join("jobs")
    }

    pub fn lock_dir(&self) -> PathBuf {
        self.cache_dir.join("locks")
    }
}

pub fn resolve_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("GRY_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    home_dir().join(".cache").join("gry")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn load_settings(cache_dir: &Path) -> RuntimeSettings {
    let path = cache_dir.join("settings.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => RuntimeSettings::default(),
    }
}

/// Parse and set one runtime setting. Returns the canonical string value.
pub fn set_setting(settings: &mut RuntimeSettings, key: &str, value: &str) -> GryResult<String> {
    let bool_value = || -> GryResult<bool> {
        match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(GryError::invalid(format!(
                "invalid boolean for {key}: {value} (use true/false)"
            ))),
        }
    };
    let u64_value = || -> GryResult<u64> {
        value
            .parse::<u64>()
            .map_err(|_| GryError::invalid(format!("invalid integer for {key}: {value}")))
    };
    let out = match key {
        "auto_index" => {
            settings.auto_index = bool_value()?;
            settings.auto_index.to_string()
        }
        "auto_index_limit" => {
            settings.auto_index_limit = u64_value()?;
            settings.auto_index_limit.to_string()
        }
        "auto_watch" => {
            settings.auto_watch = bool_value()?;
            settings.auto_watch.to_string()
        }
        "doc_candidates" => {
            if value.eq_ignore_ascii_case("auto") {
                settings.doc_candidates = None;
                "auto".to_string()
            } else {
                settings.doc_candidates = Some(bool_value()?);
                settings.doc_candidates.unwrap().to_string()
            }
        }
        "watcher_enabled" => {
            settings.watcher_enabled = bool_value()?;
            settings.watcher_enabled.to_string()
        }
        "watch_non_git" => {
            settings.watch_non_git = bool_value()?;
            settings.watch_non_git.to_string()
        }
        "index_max_files" => {
            if value.eq_ignore_ascii_case("off") {
                settings.index_max_files = None;
                "off".to_string()
            } else {
                let v = u64_value()?;
                settings.index_max_files = Some(v);
                v.to_string()
            }
        }
        "index_max_source_mb" => {
            if value.eq_ignore_ascii_case("off") {
                settings.index_max_source_mb = None;
                "off".to_string()
            } else {
                let v = u64_value()?;
                settings.index_max_source_mb = Some(v);
                v.to_string()
            }
        }
        "index_max_memory_mb" => {
            if value.eq_ignore_ascii_case("off") {
                settings.index_max_memory_mb = None;
                "off".to_string()
            } else {
                let v = u64_value()?;
                settings.index_max_memory_mb = Some(v);
                v.to_string()
            }
        }
        "semantic_enabled" => {
            settings.semantic_enabled = bool_value()?;
            settings.semantic_enabled.to_string()
        }
        other => {
            return Err(GryError::invalid(format!(
                "unknown setting: {other} (known: auto_index, auto_index_limit, auto_watch, \
                 doc_candidates, watcher_enabled, watch_non_git, index_max_files, \
                 index_max_source_mb, index_max_memory_mb, semantic_enabled)"
            )))
        }
    };
    Ok(out)
}

pub fn get_setting(settings: &RuntimeSettings, key: &str) -> GryResult<String> {
    let value = match key {
        "auto_index" => settings.auto_index.to_string(),
        "auto_index_limit" => settings.auto_index_limit.to_string(),
        "auto_watch" => settings.auto_watch.to_string(),
        "doc_candidates" => match settings.doc_candidates {
            None => "auto".to_string(),
            Some(v) => v.to_string(),
        },
        "watcher_enabled" => settings.watcher_enabled.to_string(),
        "watch_non_git" => settings.watch_non_git.to_string(),
        "index_max_files" => settings
            .index_max_files
            .map(|v| v.to_string())
            .unwrap_or_else(|| "off".to_string()),
        "index_max_source_mb" => settings
            .index_max_source_mb
            .map(|v| v.to_string())
            .unwrap_or_else(|| "off".to_string()),
        "index_max_memory_mb" => settings
            .index_max_memory_mb
            .map(|v| v.to_string())
            .unwrap_or_else(|| "off".to_string()),
        "semantic_enabled" => settings.semantic_enabled.to_string(),
        other => {
            return Err(GryError::invalid(format!(
                "unknown setting: {other} (known: auto_index, auto_index_limit, auto_watch, \
                 doc_candidates, watcher_enabled, watch_non_git, index_max_files, \
                 index_max_source_mb, index_max_memory_mb, semantic_enabled)"
            )))
        }
    };
    Ok(value)
}

pub fn reset_setting(settings: &mut RuntimeSettings, key: &str) -> GryResult<String> {
    let defaults = RuntimeSettings::default();
    match key {
        "auto_index" => settings.auto_index = defaults.auto_index,
        "auto_index_limit" => settings.auto_index_limit = defaults.auto_index_limit,
        "auto_watch" => settings.auto_watch = defaults.auto_watch,
        "doc_candidates" => settings.doc_candidates = defaults.doc_candidates,
        "watcher_enabled" => settings.watcher_enabled = defaults.watcher_enabled,
        "watch_non_git" => settings.watch_non_git = defaults.watch_non_git,
        "index_max_files" => settings.index_max_files = defaults.index_max_files,
        "index_max_source_mb" => settings.index_max_source_mb = defaults.index_max_source_mb,
        "index_max_memory_mb" => settings.index_max_memory_mb = defaults.index_max_memory_mb,
        "semantic_enabled" => settings.semantic_enabled = defaults.semantic_enabled,
        other => return Err(GryError::invalid(format!("unknown setting: {other}"))),
    }
    get_setting(settings, key)
}

pub fn all_settings(settings: &RuntimeSettings) -> Vec<(String, String)> {
    const KEYS: &[&str] = &[
        "auto_index",
        "auto_index_limit",
        "auto_watch",
        "doc_candidates",
        "watcher_enabled",
        "watch_non_git",
        "index_max_files",
        "index_max_source_mb",
        "index_max_memory_mb",
        "semantic_enabled",
    ];
    KEYS.iter()
        .map(|key| {
            (
                (*key).to_string(),
                get_setting(settings, key).unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let mut s = RuntimeSettings::default();
        assert_eq!(set_setting(&mut s, "auto_index", "true").unwrap(), "true");
        assert_eq!(get_setting(&s, "auto_index").unwrap(), "true");
        assert_eq!(
            set_setting(&mut s, "doc_candidates", "auto").unwrap(),
            "auto"
        );
        assert_eq!(
            set_setting(&mut s, "index_max_files", "off").unwrap(),
            "off"
        );
        assert_eq!(
            set_setting(&mut s, "index_max_files", "1234").unwrap(),
            "1234"
        );
        assert_eq!(get_setting(&s, "index_max_files").unwrap(), "1234");
        assert!(set_setting(&mut s, "auto_index", "maybe").is_err());
        assert!(set_setting(&mut s, "nope", "1").is_err());
        reset_setting(&mut s, "index_max_files").unwrap();
        assert_eq!(get_setting(&s, "index_max_files").unwrap(), "off");
    }

    #[test]
    fn allowed_root_refuses_escape() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let cfg = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: Some(inside.clone()),
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: RuntimeSettings::default(),
        };
        assert!(cfg.resolve_index_root(inside.to_str().unwrap()).is_ok());
        assert!(cfg.resolve_index_root(outside.to_str().unwrap()).is_err());
    }
}
