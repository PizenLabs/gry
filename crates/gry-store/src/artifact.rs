//! Team-shared graph artifact: `.gry/graph.db.zst`.
//!
//! The artifact is a compacted single-project SQLite database compressed with
//! zstd. It is an opt-in shared baseline, not a replacement for the local index.

use crate::error::StoreResult;
use crate::{now_iso, EdgeSpec, ProjectWrite, Store};
use gry_core::types::NewNode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const ARTIFACT_SCHEMA: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub schema: i64,
    pub project: String,
    pub root_path: String,
    pub generation: String,
    pub mode: String,
    pub created_at: String,
    pub sha256: String,
    pub file_count: i64,
    pub nodes: i64,
    pub edges: i64,
}

impl Store {
    /// Export one project to a compressed artifact. Returns the metadata.
    pub fn export_artifact(
        &mut self,
        project: &str,
        dest: impl AsRef<Path>,
    ) -> StoreResult<ArtifactMeta> {
        let dest = dest.as_ref();
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let record = self.require_project(project)?;
        let nodes = self.all_nodes(project)?;
        let edges = self.all_edges(project)?;
        let files = self.file_hashes(project)?;
        let coverage = self.coverage(project)?;
        let meta = self.coverage_meta(project)?;
        let parse_cache = self.parse_cache(project)?;
        let vectors = self.vectors(project)?;

        let id_to_qn: HashMap<i64, String> = nodes
            .iter()
            .map(|n| (n.id, n.qualified_name.clone()))
            .collect();
        let qn_to_id: HashMap<i64, String> = id_to_qn.clone();

        let new_nodes: Vec<NewNode> = nodes
            .iter()
            .map(|n| NewNode {
                label: n.label.clone(),
                name: n.name.clone(),
                qualified_name: n.qualified_name.clone(),
                file_path: n.file_path.clone(),
                start_line: n.start_line,
                end_line: n.end_line,
                properties: n.properties.clone(),
            })
            .collect();
        let new_edges: Vec<EdgeSpec> = edges
            .iter()
            .filter_map(|e| {
                let source = qn_to_id.get(&e.source_id)?.clone();
                let target = qn_to_id.get(&e.target_id)?.clone();
                Some(EdgeSpec {
                    source_qn: source,
                    target_qn: target,
                    edge_type: e.edge_type.clone(),
                    properties: e.properties.clone(),
                })
            })
            .collect();
        let new_vectors: Vec<(String, Vec<i8>)> = vectors
            .iter()
            .filter_map(|(id, v)| Some((id_to_qn.get(id)?.clone(), v.clone())))
            .collect();

        let tmp_dir = tempfile::tempdir()?;
        let staging = tmp_dir.path().join("artifact.db");
        let compact = tmp_dir.path().join("artifact-compact.db");
        {
            let mut staging_store = Store::open(&staging)?;
            let cov_meta = meta.clone().map(|m| crate::CoverageMeta {
                generation: m.generation,
                index_mode: m.index_mode,
                recorded_at: m.recorded_at,
                recording_status: m.recording_status,
                ignored_files_stored: m.ignored_files_stored,
                ignored_files_total: m.ignored_files_total,
                coverage_version: m.coverage_version,
                hash_records_complete: m.hash_records_complete,
            });
            let mut write =
                ProjectWrite::new(project, &record.root_path, &record.mode, &record.generation);
            write.files = &files;
            write.nodes = &new_nodes;
            write.edges = &new_edges;
            write.vectors = &new_vectors;
            write.parse_cache = &parse_cache;
            write.coverage = &coverage;
            write.coverage_meta = cov_meta.as_ref();
            staging_store.publish(&write)?;
            staging_store.checkpoint()?;
        }
        // Compact into a single self-contained file.
        {
            let conn = rusqlite::Connection::open(&staging)?;
            conn.execute("VACUUM INTO ?1", [compact.to_string_lossy().to_string()])?;
        }
        let raw = std::fs::read(&compact)?;
        let compressed = zstd::encode_all(raw.as_slice(), 9)?;
        std::fs::write(dest, &compressed)?;
        let digest = crate::sha256_hex(&compressed);
        let artifact_meta = ArtifactMeta {
            schema: ARTIFACT_SCHEMA,
            project: record.name.clone(),
            root_path: record.root_path.clone(),
            generation: record.generation,
            mode: record.mode,
            created_at: now_iso(),
            sha256: digest,
            file_count: files.len() as i64,
            nodes: nodes.len() as i64,
            edges: new_edges.len() as i64,
        };
        let sidecar = dest.with_file_name("artifact.json");
        std::fs::write(sidecar, serde_json::to_vec_pretty(&artifact_meta)?)?;
        Ok(artifact_meta)
    }

    /// Import an artifact into this store. Returns (project, nodes, edges).
    pub fn import_artifact(&mut self, artifact: impl AsRef<Path>) -> StoreResult<ArtifactMeta> {
        let artifact = artifact.as_ref();
        let compressed = std::fs::read(artifact)?;
        let raw = zstd::decode_all(compressed.as_slice())?;
        let tmp_dir = tempfile::tempdir()?;
        let db_path = tmp_dir.path().join("import.db");
        std::fs::write(&db_path, &raw)?;

        let sidecar = artifact.with_file_name("artifact.json");
        let declared: Option<ArtifactMeta> = std::fs::read(&sidecar)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(meta) = &declared {
            if meta.schema != ARTIFACT_SCHEMA {
                return Err(gry_core::GryError::invalid(format!(
                    "artifact schema {} is not supported (expected {})",
                    meta.schema, ARTIFACT_SCHEMA
                )));
            }
            let actual = crate::sha256_hex(&compressed);
            if meta.sha256 != actual {
                return Err(gry_core::GryError::invalid(
                    "artifact integrity check failed: sha256 mismatch",
                ));
            }
        }

        let src = Store::open(&db_path)?;
        let project_name = src
            .projects()?
            .into_iter()
            .next()
            .map(|p| p.name)
            .ok_or_else(|| gry_core::GryError::invalid("artifact contains no project"))?;
        let project = src.require_project(&project_name)?;
        let nodes = src.all_nodes(&project.name)?;
        let edges = src.all_edges(&project.name)?;
        let files = src.file_hashes(&project.name)?;
        let coverage = src.coverage(&project.name)?;
        let coverage_meta = src.coverage_meta(&project.name)?;
        let parse_cache = src.parse_cache(&project.name)?;
        let vectors = src.vectors(&project.name)?;
        let id_to_qn: HashMap<i64, String> = nodes
            .iter()
            .map(|n| (n.id, n.qualified_name.clone()))
            .collect();
        let new_nodes: Vec<NewNode> = nodes
            .iter()
            .map(|n| NewNode {
                label: n.label.clone(),
                name: n.name.clone(),
                qualified_name: n.qualified_name.clone(),
                file_path: n.file_path.clone(),
                start_line: n.start_line,
                end_line: n.end_line,
                properties: n.properties.clone(),
            })
            .collect();
        let new_edges: Vec<EdgeSpec> = edges
            .iter()
            .filter_map(|e| {
                Some(EdgeSpec {
                    source_qn: id_to_qn.get(&e.source_id)?.clone(),
                    target_qn: id_to_qn.get(&e.target_id)?.clone(),
                    edge_type: e.edge_type.clone(),
                    properties: e.properties.clone(),
                })
            })
            .collect();
        let new_vectors: Vec<(String, Vec<i8>)> = vectors
            .iter()
            .filter_map(|(id, v)| Some((id_to_qn.get(id)?.clone(), v.clone())))
            .collect();
        let mut write = ProjectWrite::new(
            &project.name,
            &project.root_path,
            "full",
            &project.generation,
        );
        write.files = &files;
        write.nodes = &new_nodes;
        write.edges = &new_edges;
        write.vectors = &new_vectors;
        write.parse_cache = &parse_cache;
        write.coverage = &coverage;
        let cov_meta = coverage_meta.map(|m| crate::CoverageMeta {
            generation: m.generation,
            index_mode: m.index_mode,
            recorded_at: m.recorded_at,
            recording_status: m.recording_status,
            ignored_files_stored: m.ignored_files_stored,
            ignored_files_total: m.ignored_files_total,
            coverage_version: m.coverage_version,
            hash_records_complete: m.hash_records_complete,
        });
        write.coverage_meta = cov_meta.as_ref();
        self.publish(&write)?;
        Ok(declared.unwrap_or(ArtifactMeta {
            schema: ARTIFACT_SCHEMA,
            project: project.name,
            root_path: project.root_path,
            generation: project.generation,
            mode: "full".to_string(),
            created_at: now_iso(),
            sha256: crate::sha256_hex(&compressed),
            file_count: files.len() as i64,
            nodes: nodes.len() as i64,
            edges: edges.len() as i64,
        }))
    }
}

/// Compute the canonical artifact path for a repository root.
pub fn artifact_path_for(root: &Path) -> PathBuf {
    root.join(".gry").join("graph.db.zst")
}
