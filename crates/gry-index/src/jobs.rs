//! In-process async index jobs with cancellation and observable status.

use crate::pipeline::{write_job_status, IndexReport, IndexRequest};
use gry_core::config::Config;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub project: String,
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub report: Option<IndexReport>,
}

#[derive(Clone)]
pub struct JobManager {
    jobs: Arc<Mutex<HashMap<String, Arc<Mutex<JobStatus>>>>>,
    cancel: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            cancel: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn project_name(config: &Config, req: &IndexRequest) -> gry_core::GryResult<String> {
        let root = config.resolve_index_root(&req.repo_path)?;
        Ok(req
            .name
            .as_deref()
            .map(gry_core::fqn::sanitize_project_name)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                gry_core::fqn::project_name_from_path(root.to_string_lossy().as_ref())
            }))
    }

    /// Start (or join) an async index job. Returns the current status.
    pub fn start(&self, config: Config, req: IndexRequest) -> gry_core::GryResult<JobStatus> {
        let project = Self::project_name(&config, &req)?;
        {
            let existing = {
                let jobs = self.jobs.lock();
                jobs.get(&project).map(|s| s.lock().clone())
            };
            if let Some(status) = existing {
                if matches!(status.state.as_str(), "queued" | "running" | "cancelling") {
                    return Ok(status);
                }
            }
        }
        let status = Arc::new(Mutex::new(JobStatus {
            project: project.clone(),
            state: "queued".to_string(),
            started_at: gry_store::now_iso(),
            finished_at: None,
            error: None,
            report: None,
        }));
        self.jobs.lock().insert(project.clone(), status.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel.lock().insert(project.clone(), cancel.clone());
        write_job_status(
            &config,
            &project,
            &serde_json::to_value(status.lock().clone()).unwrap_or_default(),
        );
        let jobs = self.jobs.clone();
        let project_for_thread = project.clone();
        let status_for_thread = status.clone();
        std::thread::Builder::new()
            .name(format!("gry-index-{project_for_thread}"))
            .spawn(move || {
                {
                    let mut s = status_for_thread.lock();
                    s.state = "running".to_string();
                    write_job_status(
                        &config,
                        &project_for_thread,
                        &serde_json::to_value(s.clone()).unwrap_or_default(),
                    );
                }
                let indexer = crate::pipeline::Indexer::new(config.clone());
                let result = indexer.index_with_cancel(&req, cancel.clone());
                let mut s = status_for_thread.lock();
                match result {
                    Ok(report) => {
                        s.state = "succeeded".to_string();
                        s.report = Some(report);
                    }
                    Err(gry_core::GryError::Cancelled) => {
                        s.state = "cancelled".to_string();
                    }
                    Err(e) => {
                        s.state = "failed".to_string();
                        s.error = Some(e.to_string());
                    }
                }
                s.finished_at = Some(gry_store::now_iso());
                write_job_status(
                    &config,
                    &project_for_thread,
                    &serde_json::to_value(s.clone()).unwrap_or_default(),
                );
                // Keep the terminal status so `status: true` can report
                // succeeded/failed/cancelled after the worker returns.
                let _ = &jobs;
            })
            .map_err(|e| gry_core::GryError::msg(format!("failed to start index job: {e}")))?;
        let out = status.lock().clone();
        Ok(out)
    }

    pub fn status(&self, project: &str) -> Option<JobStatus> {
        self.jobs.lock().get(project).map(|s| s.lock().clone())
    }

    pub fn cancel(&self, project: &str) -> bool {
        let running = self
            .jobs
            .lock()
            .get(project)
            .map(|s| {
                let mut s = s.lock();
                if matches!(s.state.as_str(), "queued" | "running") {
                    if let Some(flag) = self.cancel.lock().get(project) {
                        flag.store(true, Ordering::Relaxed);
                    }
                    s.state = "cancelling".to_string();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        running
    }

    pub fn active(&self) -> Vec<JobStatus> {
        self.jobs
            .lock()
            .values()
            .map(|s| s.lock().clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn failed_job_state_is_retained() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            cache_dir: dir.path().join("cache"),
            db_path: dir.path().join("cache/graph.db"),
            allowed_root: None,
            workers: 1,
            max_files: None,
            max_source_mb: None,
            settings: gry_core::config::RuntimeSettings::default(),
        };
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("a.rs"), "fn a() {}").unwrap();
        std::fs::create_dir_all(dir.path().join("cache/graph.db")).unwrap();
        let manager = JobManager::new();
        let request = IndexRequest::new(repo.to_string_lossy());
        let started = manager.start(config, request).unwrap();
        assert!(matches!(started.state.as_str(), "queued" | "running"));
        let mut state = started.state;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(status) = manager.status(&started.project) {
                state = status.state;
                if matches!(state.as_str(), "failed" | "succeeded" | "cancelled") {
                    break;
                }
            }
        }
        assert_eq!(state, "failed");
        assert!(manager
            .status(&started.project)
            .and_then(|s| s.error)
            .is_some());
    }
}
