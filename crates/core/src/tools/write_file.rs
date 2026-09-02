use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};

use super::approval::{ApprovalPreview, ToolRisk};
use super::prepared::{PreImage, PreparedKind, PreparedToolCall};
use super::project::ProjectRoot;
use super::Tool;

const MAX_BYTES: usize = 1024 * 1024;
const PREVIEW_HUNK_LINES: usize = 48;
const PREVIEW_CHARS: usize = 6_000;
const PREVIEW_MAX_HUNKS: usize = 8;

/// Create or overwrite a UTF-8 text file inside the project root.
///
/// Gated as [`ToolRisk::Write`]. [`WriteFile::prepare`] builds a
/// [`PreparedToolCall`] once (diff + BLAKE3 pre-image); execution re-checks
/// the fingerprint and writes atomically.
pub struct WriteFile {
    root: ProjectRoot,
}

impl WriteFile {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }

    fn parse_input(input: &Value) -> Result<(&str, &str), String> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `path`".to_string())?;
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `content`".to_string())?;
        if content.len() > MAX_BYTES {
            return Err(format!(
                "content is {} bytes; max is {MAX_BYTES}",
                content.len()
            ));
        }
        Ok((path, content))
    }

    /// Read the existing target for prepare. Missing is ok; unreadable or
    /// non-UTF-8 overwrite targets are rejected.
    fn read_existing(path: &Path) -> Result<Option<String>, String> {
        match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot stat overwrite target: {e}")),
            Ok(meta) if meta.is_dir() => Err("target is a directory, not a file".into()),
            Ok(_) => {
                let bytes = std::fs::read(path)
                    .map_err(|e| format!("cannot read overwrite target: {e}"))?;
                let text = String::from_utf8(bytes).map_err(|_| {
                    "overwrite target is not valid UTF-8; refusing to treat it as empty".to_string()
                })?;
                Ok(Some(text))
            }
        }
    }

    pub fn prepare_call(&self, input: Value) -> Result<PreparedToolCall, String> {
        let (path, content) = Self::parse_input(&input)?;
        let resolved = self.root.resolve_for_write(path)?;
        let rel = self.root.relativize(&resolved);
        let old = Self::read_existing(&resolved)?;
        let existed = old.is_some();
        let old_text = old.as_deref().unwrap_or("");
        let preimage = match &old {
            None => PreImage::Absent,
            Some(text) => PreImage::of_bytes(text.as_bytes()),
        };
        let diff = bounded_unified_diff(&rel, old_text, content, existed);
        let summary = if existed {
            format!(
                "Overwrite {rel} ({} → {} bytes)",
                old_text.len(),
                content.len()
            )
        } else {
            format!("Create {rel} ({} bytes)", content.len())
        };
        Ok(PreparedToolCall::write_file(
            resolved,
            rel.clone(),
            content.to_string(),
            preimage,
            ApprovalPreview {
                path: rel,
                summary,
                diff,
            },
        ))
    }

    pub(crate) fn verify_preimage(path: &Path, expected: &PreImage) -> Result<(), String> {
        let current = match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(format!(
                    "target changed before write (cannot stat): {e}; fresh approval required"
                ));
            }
            Ok(meta) if meta.is_dir() => {
                return Err(
                    "target changed before write (now a directory); fresh approval required".into(),
                );
            }
            Ok(_) => {
                let bytes = std::fs::read(path).map_err(|e| {
                    format!(
                        "target changed before write (cannot read): {e}; fresh approval required"
                    )
                })?;
                Some(bytes)
            }
        };

        match (expected, current) {
            (PreImage::Absent, None) => Ok(()),
            (PreImage::Absent, Some(_)) => Err(
                "target appeared after approval; aborting write — fresh approval required".into(),
            ),
            (PreImage::Present { .. }, None) => Err(
                "target was removed after approval; aborting write — fresh approval required"
                    .into(),
            ),
            (PreImage::Present { blake3 }, Some(bytes)) => {
                let now = *blake3::hash(&bytes).as_bytes();
                if now != *blake3 {
                    Err(
                        "target contents changed after approval; aborting write — fresh approval required"
                            .into(),
                    )
                } else {
                    Ok(())
                }
            }
        }
    }

    fn execute_write(&self, prepared: PreparedToolCall) -> Result<String, String> {
        let relative = commit_prepared_write(&self.root, prepared)?;
        Ok(format!(
            "wrote {} ({} bytes)",
            relative.path, relative.bytes
        ))
    }
}

/// What a committed write touched, for the tool's result line.
pub(crate) struct CommittedWrite {
    pub path: String,
    pub bytes: usize,
}

/// Commit a prepared whole-file replacement.
///
/// Shared by `write_file` and `edit_file`: both arrive here holding a path, a
/// full new body, and the pre-image the user actually approved against. Every
/// re-check below exists because approval and execution are separated in time —
/// the path can be re-pointed by a symlink and the bytes can change underneath.
pub(crate) fn commit_prepared_write(
    root: &ProjectRoot,
    prepared: PreparedToolCall,
) -> Result<CommittedWrite, String> {
    let tool_name = prepared.tool_name;
    let PreparedKind::WriteFile {
        absolute_path,
        relative_path,
        content,
        preimage,
    } = prepared.kind
    else {
        return Err(format!(
            "internal error: {tool_name} prepared kind mismatch"
        ));
    };

    // Re-resolve and require the same normalized path.
    let again = root.resolve_for_write(&relative_path)?;
    if again != absolute_path {
        return Err(
            "target path changed after approval (symlink or root moved); aborting write — fresh approval required"
                .into(),
        );
    }

    WriteFile::verify_preimage(&absolute_path, &preimage)?;

    if let Some(parent) = absolute_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create parent dirs failed: {e}"))?;
        // Parent must still be inside the project after create.
        let canon_parent = std::fs::canonicalize(parent)
            .map_err(|e| format!("cannot verify parent directory: {e}"))?;
        if !canon_parent.starts_with(root.as_path()) {
            return Err("parent directory resolves outside the project root".into());
        }
    }

    let bytes = content.len();
    atomic_write(&absolute_path, content.as_bytes())?;
    Ok(CommittedWrite {
        path: relative_path,
        bytes,
    })
}

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create a new UTF-8 text file in the project, or fully replace one. \
         Paths are relative to the project root. To change part of a file that \
         already exists, use `edit_file` instead — it is far cheaper and cannot \
         clobber the parts you did not intend to touch. Requires user approval \
         before the write runs."
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to the project root, e.g. src/main.rs"
                },
                "content": {
                    "type": "string",
                    "description": "Full new contents of the file"
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        self.prepare_call(input)
    }

    async fn execute_prepared(
        &self,
        prepared: PreparedToolCall,
    ) -> std::result::Result<super::ToolOutcome, String> {
        // Sync filesystem work; keep off the runtime if it ever grows heavy.
        let tool_root = self.root.clone();
        let this = Self { root: tool_root };
        tokio::task::spawn_blocking(move || this.execute_write(prepared))
            .await
            .map_err(|e| format!("write task failed: {e}"))?
            .map(super::ToolOutcome::text)
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let prepared = self.prepare_call(input)?;
        self.execute_prepared(prepared).await
    }
}

/// Write `data` via centralized atomic persistence (unique temp, flush/sync,
/// Windows `MoveFileExW` replace without deleting the destination first).
pub fn atomic_write(target: &Path, data: &[u8]) -> Result<(), String> {
    crate::fsutil::atomic_write(target, data).map_err(|e| e.to_string())
}

/// Real unified hunks via `similar`, bounded by line/char budgets. Always
/// includes changed content when present; clearly reports omitted hunks.
pub fn bounded_unified_diff(path: &str, old: &str, new: &str, existed: bool) -> String {
    let mut out = String::new();
    if existed {
        out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    } else {
        out.push_str(&format!("--- /dev/null\n+++ b/{path}\n"));
    }

    let diff = TextDiff::from_lines(old, new);
    if old.is_empty() && new.is_empty() {
        out.push_str("@@ empty file @@\n");
        return out;
    }

    let groups = diff.grouped_ops(3);
    if groups.is_empty() {
        out.push_str("@@ no changes @@\n");
        return out;
    }

    let mut hunks_shown = 0usize;
    let mut hunks_omitted = 0usize;
    let mut lines_shown = 0usize;
    let mut any_delete = false;
    let mut any_insert = false;

    for group in &groups {
        if hunks_shown >= PREVIEW_MAX_HUNKS
            || lines_shown >= PREVIEW_HUNK_LINES
            || out.len() >= PREVIEW_CHARS
        {
            hunks_omitted += 1;
            continue;
        }

        let (old_start, old_count, new_start, new_count) = hunk_extents(group);
        out.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
        ));

        // Materialize the hunk so we can prefer change lines when the budget
        // is tight (never ship a preview of only context / only one side).
        let mut changes: Vec<(ChangeTag, String)> = Vec::new();
        for op in group {
            for change in diff.iter_changes(op) {
                let mut line = change.value().to_string();
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                changes.push((change.tag(), line));
            }
        }

        let hunk_deletes = changes
            .iter()
            .filter(|(t, _)| *t == ChangeTag::Delete)
            .count();
        let hunk_inserts = changes
            .iter()
            .filter(|(t, _)| *t == ChangeTag::Insert)
            .count();
        let mut shown_del = 0usize;
        let mut shown_ins = 0usize;
        let mut truncated_mid_hunk = false;

        // Reserve room so both sides of a replace can appear.
        let reserve_for_other = if hunk_deletes > 0 && hunk_inserts > 0 {
            (PREVIEW_HUNK_LINES / 4).max(2)
        } else {
            0
        };

        for (tag, line) in &changes {
            let is_change = matches!(tag, ChangeTag::Delete | ChangeTag::Insert);
            let budget_left = PREVIEW_HUNK_LINES.saturating_sub(lines_shown);
            let chars_full = out.len() >= PREVIEW_CHARS;

            // Drop context when the budget is nearly spent.
            if !is_change && (budget_left <= reserve_for_other || chars_full) {
                continue;
            }

            // Keep a few slots for the opposite side of a replace.
            if *tag == ChangeTag::Delete
                && hunk_inserts > 0
                && shown_ins == 0
                && budget_left <= reserve_for_other
                && shown_del > 0
            {
                continue;
            }

            if lines_shown >= PREVIEW_HUNK_LINES || chars_full {
                truncated_mid_hunk = true;
                break;
            }

            let prefix = match tag {
                ChangeTag::Delete => {
                    shown_del += 1;
                    any_delete = true;
                    '-'
                }
                ChangeTag::Insert => {
                    shown_ins += 1;
                    any_insert = true;
                    '+'
                }
                ChangeTag::Equal => ' ',
            };
            out.push(prefix);
            out.push_str(line);
            out.push('\n');
            lines_shown += 1;
        }

        // If we still have not shown inserts (or deletes) that exist in this
        // hunk, append a few from the remainder so the preview always has
        // changed content on both sides when both sides changed.
        if shown_ins == 0 && hunk_inserts > 0 {
            for (tag, line) in &changes {
                if *tag != ChangeTag::Insert {
                    continue;
                }
                if lines_shown >= PREVIEW_HUNK_LINES || out.len() >= PREVIEW_CHARS {
                    truncated_mid_hunk = true;
                    break;
                }
                out.push('+');
                out.push_str(line);
                out.push('\n');
                lines_shown += 1;
                shown_ins += 1;
                any_insert = true;
                if shown_ins >= reserve_for_other.max(2) {
                    truncated_mid_hunk = true;
                    break;
                }
            }
        }
        if shown_del == 0 && hunk_deletes > 0 {
            for (tag, line) in &changes {
                if *tag != ChangeTag::Delete {
                    continue;
                }
                if lines_shown >= PREVIEW_HUNK_LINES || out.len() >= PREVIEW_CHARS {
                    truncated_mid_hunk = true;
                    break;
                }
                out.push('-');
                out.push_str(line);
                out.push('\n');
                lines_shown += 1;
                shown_del += 1;
                any_delete = true;
                if shown_del >= reserve_for_other.max(2) {
                    truncated_mid_hunk = true;
                    break;
                }
            }
        }

        if truncated_mid_hunk
            || shown_del + shown_ins
                < changes
                    .iter()
                    .filter(|(t, _)| matches!(t, ChangeTag::Delete | ChangeTag::Insert))
                    .count()
        {
            out.push_str("… hunk truncated\n");
        }

        hunks_shown += 1;
        if lines_shown >= PREVIEW_HUNK_LINES || out.len() >= PREVIEW_CHARS {
            hunks_omitted += groups.len().saturating_sub(hunks_shown);
            break;
        }
    }

    if hunks_omitted > 0 {
        out.push_str(&format!("… {hunks_omitted} hunk(s) omitted from preview\n"));
    } else if lines_shown >= PREVIEW_HUNK_LINES || out.len() >= PREVIEW_CHARS {
        // Single large hunk truncated — still a clear omission signal.
        if !out.contains("omitted") && !out.contains("truncated") {
            out.push_str("… remaining changes omitted from preview\n");
        }
    }

    debug_assert!(
        !(old.lines().count() > 0 && new.lines().count() > 0 && old != new)
            || any_delete
            || any_insert
            || out.contains("no changes")
    );
    out
}

fn hunk_extents(ops: &[similar::DiffOp]) -> (usize, usize, usize, usize) {
    let Some(first) = ops.first() else {
        return (0, 0, 0, 0);
    };
    let last = ops.last().unwrap_or(first);
    let old_range_start = first.old_range().start;
    let old_range_end = last.old_range().end;
    let new_range_start = first.new_range().start;
    let new_range_end = last.new_range().end;
    let old_count = old_range_end.saturating_sub(old_range_start);
    let new_count = new_range_end.saturating_sub(new_range_start);
    // Unified diff: 1-based starts; empty side uses the line before (0 if at start).
    let old_start = if old_count == 0 {
        old_range_start
    } else {
        old_range_start + 1
    };
    let new_start = if new_count == 0 {
        new_range_start
    } else {
        new_range_start + 1
    };
    (old_start, old_count, new_start, new_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{register_write_tools, ToolRegistry};

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-write-file-{name}-"))
    }

    #[tokio::test]
    async fn writes_new_file() {
        let dir = scratch("new");
        let tool = WriteFile::new(&dir).unwrap();
        let out = tool
            .run(json!({ "path": "note.txt", "content": "hello" }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("wrote note.txt"));
        assert_eq!(
            std::fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn preview_for_overwrite_includes_hunks() {
        let dir = scratch("preview");
        std::fs::write(dir.join("a.txt"), "old\nkeep\n").unwrap();
        let tool = WriteFile::new(&dir).unwrap();
        let prepared = tool
            .prepare_call(json!({ "path": "a.txt", "content": "new\nkeep\n" }))
            .unwrap();
        assert_eq!(prepared.preview.path, "a.txt");
        assert!(
            prepared.preview.diff.contains("-old"),
            "{}",
            prepared.preview.diff
        );
        assert!(
            prepared.preview.diff.contains("+new"),
            "{}",
            prepared.preview.diff
        );
        assert!(
            prepared.preview.diff.contains("@@"),
            "{}",
            prepared.preview.diff
        );
        assert!(prepared.preview.summary.contains("Overwrite"));
    }

    #[tokio::test]
    async fn rejects_non_utf8_overwrite_target() {
        let dir = scratch("bin");
        std::fs::write(dir.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        let tool = WriteFile::new(&dir).unwrap();
        let err = tool
            .prepare_call(json!({ "path": "bin.dat", "content": "x" }))
            .unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[tokio::test]
    async fn aborts_when_contents_change_after_prepare() {
        let dir = scratch("stale");
        std::fs::write(dir.join("a.txt"), "v1\n").unwrap();
        let tool = WriteFile::new(&dir).unwrap();
        let prepared = tool
            .prepare_call(json!({ "path": "a.txt", "content": "v2\n" }))
            .unwrap();
        std::fs::write(dir.join("a.txt"), "changed underneath\n").unwrap();
        let err = tool.execute_prepared(prepared).await.unwrap_err();
        assert!(err.contains("changed after approval"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "changed underneath\n"
        );
    }

    #[tokio::test]
    async fn aborts_when_absent_file_appears() {
        let dir = scratch("appear");
        let tool = WriteFile::new(&dir).unwrap();
        let prepared = tool
            .prepare_call(json!({ "path": "new.txt", "content": "x" }))
            .unwrap();
        std::fs::write(dir.join("new.txt"), "race\n").unwrap();
        let err = tool.execute_prepared(prepared).await.unwrap_err();
        assert!(err.contains("appeared after approval"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("new.txt")).unwrap(),
            "race\n"
        );
    }

    #[tokio::test]
    async fn atomic_write_replaces_existing() {
        let dir = scratch("atomic");
        let path = dir.join("f.txt");
        std::fs::write(&path, "old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[tokio::test]
    async fn large_diff_reports_omitted_hunks() {
        let mut old = String::new();
        let mut new = String::new();
        for i in 0..200 {
            old.push_str(&format!("line-{i}-aaa\n"));
            new.push_str(&format!("line-{i}-bbb\n"));
        }
        let diff = bounded_unified_diff("big.txt", &old, &new, true);
        assert!(diff.contains("-line-"), "{diff}");
        assert!(diff.contains("+line-"), "{diff}");
        assert!(
            diff.contains("omitted") || diff.contains("truncated"),
            "{diff}"
        );
    }

    #[tokio::test]
    async fn rejects_escape_and_missing_fields() {
        let dir = scratch("bad");
        let tool = WriteFile::new(&dir).unwrap();
        let err = tool.run(json!({ "path": "x.txt" })).await.unwrap_err();
        assert!(err.contains("content"), "{err}");
        let err = tool
            .run(json!({ "path": "../x.txt", "content": "no" }))
            .await
            .unwrap_err();
        assert!(err.contains("escapes") || err.contains("outside"), "{err}");
    }

    #[tokio::test]
    async fn registry_marks_write_risk() {
        let dir = scratch("reg");
        let mut reg = ToolRegistry::new();
        register_write_tools(&mut reg, &dir).unwrap();
        assert_eq!(reg.risk("write_file"), Some(ToolRisk::Write));
        let prepared = reg
            .prepare("write_file", json!({ "path": "f.txt", "content": "x" }))
            .unwrap();
        assert_eq!(prepared.risk, ToolRisk::Write);
        assert!(!prepared.preview.path.is_empty());
    }
}
