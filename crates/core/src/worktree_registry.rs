//! Registry shared by desktop workspaces, including legacy project mappings.
use crate::display_path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeChatRecord {
    pub path: PathBuf,
    pub source_root: PathBuf,
    pub branch: String,
    #[serde(default = "worktree_visible_default")]
    pub visible: bool,
    /// The commit the checkout started from. Absent in records written before
    /// worktree status existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
}

fn worktree_visible_default() -> bool {
    true
}

pub fn worktree_storage_root(config_dir: &Path) -> PathBuf {
    let storage = config_dir.join("worktrees");
    storage.canonicalize().unwrap_or(storage)
}

pub fn canonical_managed_worktree_path(config_dir: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let storage = worktree_storage_root(config_dir);
    let canonical = path.canonicalize().ok()?;
    (canonical != storage && canonical.starts_with(&storage)).then_some(canonical)
}

pub fn worktree_record_path(config_dir: &Path, root: &Path) -> PathBuf {
    let key = display_path(root);
    let key = if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    };
    config_dir
        .join("worktree-chat-records")
        .join(format!("{}.json", blake3::hash(key.as_bytes()).to_hex()))
}

pub fn save_worktree_chat_record_at(
    config_dir: &Path,
    record: &WorktreeChatRecord,
) -> Result<(), String> {
    let path = worktree_record_path(config_dir, &record.path);
    std::fs::create_dir_all(path.parent().expect("record path has a parent"))
        .map_err(|error| format!("cannot prepare worktree chat registry: {error}"))?;
    crate::fsutil::atomic_write_json(&path, record)
        .map_err(|error| format!("cannot save worktree chat registry: {error}"))
}

pub fn load_worktree_chat_records_from(config_dir: &Path) -> Vec<WorktreeChatRecord> {
    let mut by_path = BTreeMap::<PathBuf, WorktreeChatRecord>::new();
    // Older builds wrote one shared array. Keep reading it, but a bad array
    // cannot prevent ordinary projects or independently saved records opening.
    if let Ok(body) = std::fs::read_to_string(config_dir.join("worktree-chats.json")) {
        if let Ok(records) = serde_json::from_str::<Vec<WorktreeChatRecord>>(&body) {
            for mut record in records {
                if let Some(canonical_path) =
                    canonical_managed_worktree_path(config_dir, &record.path)
                {
                    let sidecar = worktree_record_path(config_dir, &canonical_path);
                    let sidecar_is_missing = std::fs::symlink_metadata(sidecar)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
                    if sidecar_is_missing {
                        record.path = canonical_path.clone();
                        by_path.insert(canonical_path, record);
                    }
                }
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(config_dir.join("worktree-chat-records")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<WorktreeChatRecord>(&body) else {
                continue;
            };
            let Some(canonical_path) = canonical_managed_worktree_path(config_dir, &record.path)
            else {
                continue;
            };
            if canonical_path == record.path
                && path == worktree_record_path(config_dir, &canonical_path)
            {
                by_path.insert(canonical_path, record);
            }
        }
    }
    by_path.into_values().collect()
}

pub fn worktree_config_root_from_records(
    root: &Path,
    records: &[WorktreeChatRecord],
    storage: &Path,
) -> Result<PathBuf, String> {
    if !root.starts_with(storage) {
        return Ok(root.to_path_buf());
    }
    let Some(record) = records.iter().find(|record| record.path == root) else {
        return Err(format!(
            "This Zest worktree has no provider mapping: {}. Restore its worktree chat record before opening it.",
            display_path(root)
        ));
    };
    if !record.source_root.is_dir() {
        return Err(format!(
            "The original project for this worktree is missing: {}",
            display_path(&record.source_root)
        ));
    }
    Ok(record.source_root.clone())
}

pub fn set_visibility(config_dir: &Path, root: &Path, visible: bool) -> Result<(), String> {
    if !root.starts_with(worktree_storage_root(config_dir)) {
        return Ok(());
    }
    let mut record = load_worktree_chat_records_from(config_dir)
        .into_iter()
        .find(|record| record.path == root)
        .ok_or_else(|| {
            format!(
                "This Zest worktree has no provider mapping: {}",
                display_path(root)
            )
        })?;
    if record.visible != visible {
        record.visible = visible;
        save_worktree_chat_record_at(config_dir, &record)?;
    }
    Ok(())
}

pub fn config_root(config_dir: &Path, root: &Path) -> Result<PathBuf, String> {
    let storage = worktree_storage_root(config_dir);
    if !root.starts_with(&storage) {
        return Ok(root.to_owned());
    }
    worktree_config_root_from_records(root, &load_worktree_chat_records_from(config_dir), &storage)
}

/// Register the original configuration before returning the checkout to a frontend.
pub fn create(config_dir: &Path, source: &Path) -> Result<WorktreeChatRecord, String> {
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let source_root = config_root(config_dir, &source)?;
    let created =
        crate::worktree_chat::create_chat_worktree(&source, &worktree_storage_root(config_dir))?;
    let record = WorktreeChatRecord {
        path: created.path.canonicalize().map_err(|e| e.to_string())?,
        source_root,
        branch: created.branch,
        base_commit: Some(created.base_commit),
        visible: true,
    };
    save_worktree_chat_record_at(config_dir, &record).map_err(|e| {
        format!(
            "Worktree created at {} but could not be registered: {e}",
            display_path(&record.path)
        )
    })?;
    Ok(record)
}
