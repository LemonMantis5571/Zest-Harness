//! Multi-hunk, multi-file structured patching inside the project root.
//!
//! `edit_file` is ideal for replacing a single string in one file, while large
//! refactors across multiple hunks or files otherwise force the model into a
//! serial chain of round-trips. `apply_patch` accepts the standard V4A
//! `*** Begin Patch` / `*** End Patch` grammar (`*** Add File:`,
//! `*** Update File:` with optional `*** Move to:`, and `*** Delete File:`),
//! computes every target state at prepare time, binds BLAKE3 pre-images for
//! every touched path, escalates to [`ToolRisk::Sensitive`] if any path is
//! sensitive, and verifies all pre-images before committing a single byte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::approval::{ApprovalPreview, ToolRisk};
use super::prepared::{
    PatchCommitAction, PreImage, PreparedKind, PreparedPatchOp, PreparedToolCall,
};
use super::project::ProjectRoot;
use super::write_file::{atomic_write, bounded_unified_diff, WriteFile};
use super::Tool;

const MAX_BYTES: usize = 1024 * 1024;
const PREVIEW_MAX_CHARS: usize = 6_000;

pub struct ApplyPatch {
    root: ProjectRoot,
}

#[derive(Debug)]
enum ParsedSection {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

#[derive(Debug, Default)]
struct PatchHunk {
    lines: Vec<HunkLine>,
    end_of_file: bool,
}

#[derive(Debug, Clone)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone)]
struct TrackedFile {
    absolute_path: PathBuf,
    preimage: PreImage,
    initial_text: Option<String>,
    current_text: Option<String>,
}

impl ApplyPatch {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }

    pub fn prepare_call(&self, input: Value) -> Result<PreparedToolCall, String> {
        let raw_patch = input
            .get("patch")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `patch`".to_string())?;
        let sections = parse_patch(raw_patch)?;
        if sections.is_empty() {
            return Err("patch contains no file operations".into());
        }

        let mut tracked: BTreeMap<String, TrackedFile> = BTreeMap::new();
        for section in sections {
            match section {
                ParsedSection::Add { path, lines } => {
                    let entry = self.load_entry(&mut tracked, &path)?;
                    if entry.current_text.is_some() {
                        return Err(format!(
                            "cannot add `{}`: file already exists (use `*** Update File:` instead)",
                            path
                        ));
                    }
                    let mut content = lines.join("\n");
                    if !lines.is_empty() {
                        content.push('\n');
                    }
                    if content.len() > MAX_BYTES {
                        return Err(format!(
                            "result for `{path}` would be {} bytes; max is {MAX_BYTES}",
                            content.len()
                        ));
                    }
                    entry.current_text = Some(content);
                }
                ParsedSection::Delete { path } => {
                    let entry = self.load_entry(&mut tracked, &path)?;
                    if entry.current_text.is_none() {
                        return Err(format!("cannot delete `{path}`: file does not exist"));
                    }
                    entry.current_text = None;
                }
                ParsedSection::Update {
                    path,
                    move_to,
                    hunks,
                } => {
                    let src_rel = {
                        let resolved = self.root.resolve_for_write(&path)?;
                        self.root.relativize(&resolved)
                    };
                    let old_text = {
                        let entry = self.load_entry(&mut tracked, &path)?;
                        entry.current_text.clone().ok_or_else(|| {
                            format!(
                                "cannot update `{src_rel}`: file does not exist (use `*** Add File:` to create it)"
                            )
                        })?
                    };
                    let new_text = apply_hunks(&src_rel, &old_text, &hunks)?;
                    if new_text.len() > MAX_BYTES {
                        return Err(format!(
                            "result for `{src_rel}` would be {} bytes; max is {MAX_BYTES}",
                            new_text.len()
                        ));
                    }
                    if let Some(dest_raw) = move_to {
                        let dest_resolved = self.root.resolve_for_write(&dest_raw)?;
                        let dest_rel = self.root.relativize(&dest_resolved);
                        if dest_rel == src_rel {
                            let entry = tracked.get_mut(&src_rel).unwrap();
                            entry.current_text = Some(new_text);
                        } else {
                            let dest_entry = self.load_entry(&mut tracked, &dest_raw)?;
                            if dest_entry.current_text.is_some() {
                                return Err(format!(
                                    "cannot move `{src_rel}` to `{dest_rel}`: destination already exists"
                                ));
                            }
                            dest_entry.current_text = Some(new_text);
                            let src_entry = tracked.get_mut(&src_rel).unwrap();
                            src_entry.current_text = None;
                        }
                    } else {
                        if new_text == old_text {
                            return Err(format!("update to `{src_rel}` leaves the file unchanged"));
                        }
                        let entry = tracked.get_mut(&src_rel).unwrap();
                        entry.current_text = Some(new_text);
                    }
                }
            }
        }

        let mut ops = Vec::new();
        let mut touched_names = Vec::new();
        let mut combined_diff = String::new();
        let mut added = 0usize;
        let mut modified = 0usize;
        let mut deleted = 0usize;

        for (rel, state) in tracked {
            if state.initial_text == state.current_text {
                continue;
            }
            touched_names.push(rel.clone());
            let file_diff = match (&state.initial_text, &state.current_text) {
                (None, Some(new)) => {
                    added += 1;
                    bounded_unified_diff(&rel, "", new, false)
                }
                (Some(old), Some(new)) => {
                    modified += 1;
                    bounded_unified_diff(&rel, old, new, true)
                }
                (Some(old), None) => {
                    deleted += 1;
                    format!(
                        "--- a/{rel}\n+++ /dev/null\n@@ deleted ({} bytes) @@\n",
                        old.len()
                    )
                }
                (None, None) => continue,
            };
            if combined_diff.len() < PREVIEW_MAX_CHARS {
                if !combined_diff.is_empty() && !combined_diff.ends_with('\n') {
                    combined_diff.push('\n');
                }
                let remaining = PREVIEW_MAX_CHARS.saturating_sub(combined_diff.len());
                if file_diff.len() <= remaining {
                    combined_diff.push_str(&file_diff);
                } else {
                    let end = file_diff
                        .char_indices()
                        .take_while(|(idx, _)| *idx < remaining)
                        .last()
                        .map_or(0, |(idx, ch)| idx + ch.len_utf8());
                    combined_diff.push_str(&file_diff[..end]);
                    combined_diff.push_str("\n... (patch preview truncated)\n");
                }
            }
            let action = match state.current_text {
                Some(content) => PatchCommitAction::Write { content },
                None => PatchCommitAction::Delete,
            };
            ops.push(PreparedPatchOp {
                absolute_path: state.absolute_path,
                relative_path: rel,
                preimage: state.preimage,
                action,
            });
        }

        if ops.is_empty() {
            return Err("patch leaves all files unchanged".into());
        }

        let preview_path = touched_names.join(", ");
        let summary = if ops.len() == 1 {
            format!("Patch {}", touched_names[0])
        } else {
            let mut counts = Vec::new();
            if added > 0 {
                counts.push(format!("{added} added"));
            }
            if modified > 0 {
                counts.push(format!("{modified} updated"));
            }
            if deleted > 0 {
                counts.push(format!("{deleted} deleted"));
            }
            format!("Patch {} files ({})", ops.len(), counts.join(", "))
        };

        Ok(PreparedToolCall::patch(
            ops,
            ApprovalPreview {
                path: preview_path,
                summary,
                diff: combined_diff,
            },
        ))
    }

    fn load_entry<'a>(
        &self,
        tracked: &'a mut BTreeMap<String, TrackedFile>,
        raw_path: &str,
    ) -> Result<&'a mut TrackedFile, String> {
        let resolved = self.root.resolve_for_write(raw_path)?;
        let rel = self.root.relativize(&resolved);
        if !tracked.contains_key(&rel) {
            let (preimage, initial_text) = match std::fs::metadata(&resolved) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (PreImage::Absent, None),
                Err(e) => return Err(format!("cannot stat `{rel}`: {e}")),
                Ok(meta) if meta.is_dir() => {
                    return Err(format!("`{rel}` is a directory, not a file"))
                }
                Ok(_) => {
                    let bytes = std::fs::read(&resolved)
                        .map_err(|e| format!("cannot read `{rel}`: {e}"))?;
                    let text = String::from_utf8(bytes).map_err(|_| {
                        format!("`{rel}` is not valid UTF-8; refusing to patch binary data")
                    })?;
                    (PreImage::of_bytes(text.as_bytes()), Some(text))
                }
            };
            tracked.insert(
                rel.clone(),
                TrackedFile {
                    absolute_path: resolved,
                    preimage,
                    current_text: initial_text.clone(),
                    initial_text,
                },
            );
        }
        Ok(tracked.get_mut(&rel).unwrap())
    }
}

fn commit_prepared_patch(root: &ProjectRoot, prepared: PreparedToolCall) -> Result<String, String> {
    let PreparedKind::Patch { ops } = prepared.kind else {
        return Err("internal error: apply_patch prepared kind mismatch".into());
    };

    // Verify every path and pre-image up front before touching the filesystem.
    for op in &ops {
        let again = root.resolve_for_write(&op.relative_path)?;
        if again != op.absolute_path {
            return Err(format!(
                "target path `{}` changed after approval; aborting patch — fresh approval required",
                op.relative_path
            ));
        }
        WriteFile::verify_preimage(&op.absolute_path, &op.preimage)
            .map_err(|err| format!("`{}`: {err}", op.relative_path))?;
    }

    let mut summary_lines = Vec::with_capacity(ops.len());

    // Commit writes first (so moves materialize their destination before the
    // source path is unlinked), then deletes.
    for op in &ops {
        if let PatchCommitAction::Write { content } = &op.action {
            if let Some(parent) = op.absolute_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!("create parent dirs for `{}` failed: {e}", op.relative_path)
                })?;
                let canon_parent = std::fs::canonicalize(parent).map_err(|e| {
                    format!(
                        "cannot verify parent directory for `{}`: {e}",
                        op.relative_path
                    )
                })?;
                if !canon_parent.starts_with(root.as_path()) {
                    return Err(format!(
                        "parent directory for `{}` resolves outside the project root",
                        op.relative_path
                    ));
                }
            }
            atomic_write(&op.absolute_path, content.as_bytes())?;
            let verb = match op.preimage {
                PreImage::Absent => "added",
                PreImage::Present { .. } => "updated",
            };
            summary_lines.push(format!(
                "{verb} {} ({} bytes)",
                op.relative_path,
                content.len()
            ));
        }
    }

    for op in &ops {
        if matches!(op.action, PatchCommitAction::Delete) {
            std::fs::remove_file(&op.absolute_path)
                .map_err(|e| format!("delete `{}` failed: {e}", op.relative_path))?;
            summary_lines.push(format!("deleted {}", op.relative_path));
        }
    }

    Ok(format!(
        "applied patch ({} file{}):\n{}",
        ops.len(),
        if ops.len() == 1 { "" } else { "s" },
        summary_lines.join("\n")
    ))
}

fn parse_patch(raw: &str) -> Result<Vec<ParsedSection>, String> {
    let normalized = raw.replace("\r\n", "\n");
    let mut lines: Vec<&str> = normalized.lines().collect();

    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    // Strip an accidental surrounding markdown code fence if present.
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
        && lines.last().is_some_and(|line| line.trim() == "```")
        && lines.len() >= 2
    {
        lines.remove(0);
        lines.pop();
        while lines.first().is_some_and(|line| line.trim().is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
    }

    if lines.first().map(|l| l.trim()) != Some("*** Begin Patch") {
        return Err("patch must start with `*** Begin Patch`".into());
    }
    if lines.last().map(|l| l.trim()) != Some("*** End Patch") {
        return Err("patch must end with `*** End Patch`".into());
    }

    let body = &lines[1..lines.len() - 1];
    let mut sections = Vec::new();
    let mut index = 0usize;

    while index < body.len() {
        let header = body[index].trim_end();
        if header.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some(rest) = header.strip_prefix("*** Add File:") {
            let path = rest.trim().to_string();
            if path.is_empty() {
                return Err("`*** Add File:` requires a relative file path".into());
            }
            index += 1;
            let mut added_lines = Vec::new();
            while index < body.len() && !is_section_header(body[index]) {
                let line = body[index];
                if let Some(content) = line.strip_prefix('+') {
                    added_lines.push(content.to_string());
                } else {
                    return Err(format!(
                        "invalid line in `*** Add File: {path}` (every content line must start with `+`): `{line}`"
                    ));
                }
                index += 1;
            }
            sections.push(ParsedSection::Add {
                path,
                lines: added_lines,
            });
        } else if let Some(rest) = header.strip_prefix("*** Delete File:") {
            let path = rest.trim().to_string();
            if path.is_empty() {
                return Err("`*** Delete File:` requires a relative file path".into());
            }
            index += 1;
            while index < body.len() && body[index].trim().is_empty() {
                index += 1;
            }
            if index < body.len() && !is_section_header(body[index]) {
                return Err(format!(
                    "unexpected content under `*** Delete File: {path}`: `{}`",
                    body[index]
                ));
            }
            sections.push(ParsedSection::Delete { path });
        } else if let Some(rest) = header.strip_prefix("*** Update File:") {
            let path = rest.trim().to_string();
            if path.is_empty() {
                return Err("`*** Update File:` requires a relative file path".into());
            }
            index += 1;
            let mut move_to = None;
            if index < body.len() {
                if let Some(dest) = body[index].trim_end().strip_prefix("*** Move to:") {
                    let dest = dest.trim().to_string();
                    if dest.is_empty() {
                        return Err("`*** Move to:` requires a destination path".into());
                    }
                    move_to = Some(dest);
                    index += 1;
                }
            }

            let mut hunks = Vec::new();
            let mut current = PatchHunk::default();
            let mut in_hunk = false;

            while index < body.len() && !is_section_header(body[index]) {
                let line = body[index];
                if line == "@@" || line.starts_with("@@ ") {
                    if in_hunk && !current.lines.is_empty() {
                        hunks.push(std::mem::take(&mut current));
                    }
                    in_hunk = true;
                    index += 1;
                    continue;
                }
                if line.trim() == "*** End of File" {
                    current.end_of_file = true;
                    index += 1;
                    continue;
                }
                if line.starts_with("\\ No newline at end of file") {
                    index += 1;
                    continue;
                }
                in_hunk = true;
                if let Some(text) = line.strip_prefix(' ') {
                    current.lines.push(HunkLine::Context(text.to_string()));
                } else if let Some(text) = line.strip_prefix('-') {
                    current.lines.push(HunkLine::Remove(text.to_string()));
                } else if let Some(text) = line.strip_prefix('+') {
                    current.lines.push(HunkLine::Add(text.to_string()));
                } else if line.is_empty() {
                    current.lines.push(HunkLine::Context(String::new()));
                } else {
                    return Err(format!(
                        "invalid hunk line in `{path}` (expected `@@`, ` `, `-`, or `+` prefix): `{line}`"
                    ));
                }
                index += 1;
            }
            if !current.lines.is_empty() {
                hunks.push(current);
            }
            if hunks.is_empty() && move_to.is_none() {
                return Err(format!("`*** Update File: {path}` contains no hunks"));
            }
            sections.push(ParsedSection::Update {
                path,
                move_to,
                hunks,
            });
        } else {
            return Err(format!(
                "unexpected patch section header `{header}` (expected `*** Add File:`, `*** Update File:`, or `*** Delete File:`)"
            ));
        }
    }

    Ok(sections)
}

fn is_section_header(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.starts_with("*** Add File:")
        || trimmed.starts_with("*** Delete File:")
        || trimmed.starts_with("*** Update File:")
}

fn apply_hunks(rel: &str, raw: &str, hunks: &[PatchHunk]) -> Result<String, String> {
    if hunks.is_empty() {
        return Ok(raw.to_string());
    }

    let total_newlines = raw.matches('\n').count();
    let crlf_count = raw.matches("\r\n").count();
    let line_ending = if total_newlines > 0 && crlf_count * 2 >= total_newlines {
        "\r\n"
    } else {
        "\n"
    };
    let had_trailing_newline = raw.ends_with('\n');

    let normalized = raw.replace("\r\n", "\n");
    let mut file_lines: Vec<String> = if normalized.is_empty() {
        Vec::new()
    } else {
        let body = normalized.strip_suffix('\n').unwrap_or(&normalized);
        body.split('\n').map(str::to_owned).collect()
    };

    let mut cursor = 0usize;
    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        let old_seq: Vec<&str> = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(s) | HunkLine::Remove(s) => Some(s.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect();
        let new_seq: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(s) | HunkLine::Add(s) => Some(s.clone()),
                HunkLine::Remove(_) => None,
            })
            .collect();

        if old_seq.is_empty() {
            let insert_at = if hunk.end_of_file {
                file_lines.len()
            } else {
                cursor.min(file_lines.len())
            };
            let added_len = new_seq.len();
            file_lines.splice(insert_at..insert_at, new_seq);
            cursor = insert_at + added_len;
            continue;
        }

        let Some(pos) = locate_hunk_lines(&file_lines, &old_seq, cursor, hunk.end_of_file)? else {
            return Err(format!(
                "hunk #{} did not match `{rel}`. Read `{rel}` with `read_file` and verify surrounding context lines.",
                hunk_idx + 1
            ));
        };

        let new_len = new_seq.len();
        file_lines.splice(pos..pos + old_seq.len(), new_seq);
        cursor = pos + new_len;
    }

    if file_lines.is_empty() {
        return Ok(String::new());
    }
    let mut joined = file_lines.join(line_ending);
    if had_trailing_newline {
        joined.push_str(line_ending);
    }
    Ok(joined)
}

fn locate_hunk_lines(
    file_lines: &[String],
    pattern: &[&str],
    cursor: usize,
    end_of_file: bool,
) -> Result<Option<usize>, String> {
    if pattern.len() > file_lines.len() {
        return Ok(None);
    }
    if end_of_file {
        let start = file_lines.len() - pattern.len();
        if lines_match(&file_lines[start..], pattern, false)
            || lines_match(&file_lines[start..], pattern, true)
        {
            return Ok(Some(start));
        }
    }

    // 1. Exact line comparison from cursor forward, then from 0..cursor if unique.
    if let Some(pos) = find_window(file_lines, pattern, cursor, false)? {
        return Ok(Some(pos));
    }
    // 2. Fallback ignoring trailing whitespace, requiring an unambiguous match.
    find_window(file_lines, pattern, cursor, true)
}

fn find_window(
    file_lines: &[String],
    pattern: &[&str],
    cursor: usize,
    trim_end: bool,
) -> Result<Option<usize>, String> {
    let last_start = file_lines.len() - pattern.len();
    let forward: Vec<usize> = (cursor.min(last_start + 1)..=last_start)
        .filter(|&start| lines_match(&file_lines[start..start + pattern.len()], pattern, trim_end))
        .collect();
    if forward.len() == 1 {
        return Ok(Some(forward[0]));
    }
    if forward.len() > 1 {
        return Err(format!(
            "hunk context matched {} places; include more surrounding context lines to make it unique",
            forward.len()
        ));
    }

    let all: Vec<usize> = (0..=last_start)
        .filter(|&start| lines_match(&file_lines[start..start + pattern.len()], pattern, trim_end))
        .collect();
    match all.len() {
        0 => Ok(None),
        1 => Ok(Some(all[0])),
        n => Err(format!(
            "hunk context matched {n} places; include more surrounding context lines to make it unique"
        )),
    }
}

fn lines_match(window: &[String], pattern: &[&str], trim_end: bool) -> bool {
    window.iter().zip(pattern).all(|(a, b)| {
        if trim_end {
            a.trim_end() == b.trim_end()
        } else {
            a.as_str() == *b
        }
    })
}

#[async_trait]
impl Tool for ApplyPatch {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a multi-hunk or multi-file patch inside the project root in one \
         atomic operation. Use `*** Begin Patch` ... `*** End Patch` with \
         `*** Add File: <path>`, `*** Update File: <path>` (optional \
         `*** Move to: <new_path>` and `@@` hunks of ` ` context, `-` removed, \
         and `+` added lines), and `*** Delete File: <path>`. Prefer this over \
         issuing a long chain of sequential `edit_file` calls when modifying \
         several sections or files together. Every touched path is checked \
         against its pre-image before any file is written or deleted."
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Structured patch text starting with `*** Begin Patch` and ending with `*** End Patch`."
                }
            },
            "required": ["patch"],
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
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || commit_prepared_patch(&root, prepared))
            .await
            .map_err(|e| format!("apply_patch task failed: {e}"))?
            .map(super::ToolOutcome::text)
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let prepared = self.prepare_call(input)?;
        self.execute_prepared(prepared).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-apply-patch-{name}-"))
    }

    #[tokio::test]
    async fn applies_multi_file_add_update_and_delete_atomically() {
        let dir = scratch("multi");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn alpha() -> i32 {\n    1\n}\n\npub fn beta() -> i32 {\n    2\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("obsolete.txt"), "remove me\n").unwrap();

        let patch = "\
*** Begin Patch
*** Update File: src/lib.rs
@@
 pub fn alpha() -> i32 {
-    1
+    10
 }
@@
 pub fn beta() -> i32 {
-    2
+    20
 }
*** Add File: src/extra.rs
+pub const EXTRA: i32 = 30;
*** Delete File: obsolete.txt
*** End Patch";

        let tool = ApplyPatch::new(&dir).unwrap();
        let out = tool.run(json!({ "patch": patch })).await.unwrap().body;
        assert!(out.contains("applied patch (3 files)"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("src/lib.rs")).unwrap(),
            "pub fn alpha() -> i32 {\n    10\n}\n\npub fn beta() -> i32 {\n    20\n}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("src/extra.rs")).unwrap(),
            "pub const EXTRA: i32 = 30;\n"
        );
        assert!(!dir.join("obsolete.txt").exists());
    }

    #[tokio::test]
    async fn preserves_crlf_line_endings_when_updating() {
        let dir = scratch("crlf");
        std::fs::write(dir.join("win.rs"), "fn a() {\r\n    let v = 1;\r\n}\r\n").unwrap();
        let patch = "\
*** Begin Patch
*** Update File: win.rs
@@
 fn a() {
-    let v = 1;
+    let v = 2;
 }
*** End Patch";
        let tool = ApplyPatch::new(&dir).unwrap();
        tool.run(json!({ "patch": patch })).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("win.rs")).unwrap(),
            "fn a() {\r\n    let v = 2;\r\n}\r\n"
        );
    }

    #[test]
    fn touching_any_sensitive_file_escalates_whole_patch_to_sensitive() {
        let dir = scratch("sensitive");
        std::fs::write(dir.join("safe.rs"), "let x = 1;\n").unwrap();
        std::fs::write(dir.join(".env"), "API_KEY=old\n").unwrap();

        let patch = "\
*** Begin Patch
*** Update File: safe.rs
@@
-let x = 1;
+let x = 2;
*** Update File: .env
@@
-API_KEY=old
+API_KEY=new
*** End Patch";

        let tool = ApplyPatch::new(&dir).unwrap();
        let prepared = tool.prepare_call(json!({ "patch": patch })).unwrap();
        assert_eq!(prepared.risk, ToolRisk::Sensitive);
    }

    #[tokio::test]
    async fn stale_preimage_on_second_file_aborts_before_modifying_first_file() {
        let dir = scratch("preimage");
        std::fs::write(dir.join("first.txt"), "one\n").unwrap();
        std::fs::write(dir.join("second.txt"), "two\n").unwrap();

        let patch = "\
*** Begin Patch
*** Update File: first.txt
@@
-one
+ONE
*** Update File: second.txt
@@
-two
+TWO
*** End Patch";

        let tool = ApplyPatch::new(&dir).unwrap();
        let prepared = tool.prepare_call(json!({ "patch": patch })).unwrap();

        // Concurrent modification after prepare/approval.
        std::fs::write(dir.join("second.txt"), "changed underneath\n").unwrap();

        let err = tool.execute_prepared(prepared).await.unwrap_err();
        assert!(err.contains("fresh approval required"), "{err}");
        // Neither file may have been mutated!
        assert_eq!(
            std::fs::read_to_string(dir.join("first.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("second.txt")).unwrap(),
            "changed underneath\n"
        );
    }

    #[test]
    fn rejects_path_escape() {
        let dir = scratch("escape");
        let patch = "\
*** Begin Patch
*** Add File: ../escaped.txt
+nope
*** End Patch";
        let tool = ApplyPatch::new(&dir).unwrap();
        let err = tool.prepare_call(json!({ "patch": patch })).unwrap_err();
        assert!(err.contains("escapes") || err.contains("outside"), "{err}");
    }
}
