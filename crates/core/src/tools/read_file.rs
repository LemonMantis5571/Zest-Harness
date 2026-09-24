use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::approval::{ApprovalPreview, ToolRisk};
use super::prepared::PreparedToolCall;
use super::project::ProjectRoot;
use super::sensitive::is_sensitive_path;
use super::Tool;

/// This tool's own name, shared so a policy that must exempt it cannot drift
/// from the string the registry dispatches on.
pub const READ_FILE_TOOL: &str = "read_file";

/// Maximum retained text bytes for one requested window. The reader still scans
/// the file to count lines, so later offsets remain reachable without buffering
/// the whole file.
pub const MAX_BYTES: usize = 256 * 1024;
/// Lines returned when the call does not ask for a narrower window.
const DEFAULT_LINE_LIMIT: usize = 2_000;

/// Read a text file, confined to a project root.
///
/// The path in a tool call is model output, not user input — it gets the same
/// treatment as anything else off the wire. Every path is canonicalized and
/// checked against the root before it reaches the filesystem, which closes
/// `..`, absolute paths, and symlinks pointing outside the tree.
///
/// Output is line-numbered so the model has stable anchors to quote back to
/// `edit_file`. The numbers are display chrome: every truncation is announced
/// with the range that was returned and the total, because a model that
/// believes it saw a whole file will happily edit against the part it missed.
///
/// Likely-secret files require per-call approval; discovery tools omit them.
pub struct ReadFile {
    root: ProjectRoot,
}

impl ReadFile {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }

    fn prepare_call(&self, input: Value) -> Result<PreparedToolCall, String> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `path`".to_string())?;

        let resolved = self.root.resolve(path)?;
        let rel = self.root.relativize(&resolved);

        if is_sensitive_path(&rel) {
            return Ok(PreparedToolCall::plain_with_preview(
                "read_file",
                ToolRisk::Sensitive,
                input,
                ApprovalPreview {
                    path: rel.clone(),
                    summary: format!("Read sensitive file {rel}"),
                    diff: String::new(),
                },
            ));
        }

        Ok(PreparedToolCall::plain("read_file", ToolRisk::Read, input))
    }

    async fn read_path(
        &self,
        path: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String, String> {
        let resolved = self.root.resolve(path)?;
        let offset = offset.unwrap_or(1).max(1);
        let limit = limit.unwrap_or(DEFAULT_LINE_LIMIT).max(1);
        tokio::task::spawn_blocking(move || {
            let window = collect_lines_window(&resolved, offset, limit)?;
            Ok(render_lines(
                &window.lines,
                window.total_lines,
                offset,
                limit,
                window.truncated,
            ))
        })
        .await
        .map_err(|e| format!("read worker failed: {e}"))?
    }
}

/// Stream the complete file to count lines while retaining only the requested
/// window, capped at `MAX_BYTES`. Large files remain pageable without an
/// allocation proportional to their size.
struct LineWindow {
    lines: Vec<(usize, String)>,
    total_lines: usize,
    truncated: bool,
}

fn collect_lines_window(path: &PathBuf, start: usize, limit: usize) -> Result<LineWindow, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut reader = BufReader::new(file);
    let end = start.saturating_add(limit);
    let mut line_number = 1usize;
    let mut line_bytes = 0usize;
    let mut retained_bytes = 0usize;
    let mut line = Vec::new();
    let mut lines = Vec::new();
    let mut truncated = false;
    let mut line_truncated = false;

    loop {
        let (consumed, newline, empty) = {
            let buffer = reader.fill_buf().map_err(|e| format!("read failed: {e}"))?;
            if buffer.is_empty() {
                (0, false, true)
            } else if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
                (position + 1, true, false)
            } else {
                (buffer.len(), false, false)
            }
        };
        if empty {
            if line_bytes > 0 {
                if (start..end).contains(&line_number) {
                    finish_window_line(&mut lines, &mut line, line_number, !line_truncated);
                }
                line_number = line_number.saturating_add(1);
            }
            break;
        }

        let content_bytes = consumed - usize::from(newline);
        if (start..end).contains(&line_number) {
            let available = MAX_BYTES.saturating_sub(retained_bytes);
            let take = available.min(content_bytes);
            let buffer = reader.fill_buf().map_err(|e| format!("read failed: {e}"))?;
            line.extend_from_slice(&buffer[..take]);
            retained_bytes += take;
            if take < content_bytes {
                truncated = true;
                line_truncated = true;
            }
        }
        line_bytes = line_bytes.saturating_add(content_bytes);
        reader.consume(consumed);

        if newline {
            if (start..end).contains(&line_number) {
                finish_window_line(&mut lines, &mut line, line_number, !line_truncated);
            }
            line_number = line_number.saturating_add(1);
            line_bytes = 0;
            line_truncated = false;
        }
    }

    Ok(LineWindow {
        lines,
        total_lines: line_number.saturating_sub(1),
        truncated,
    })
}

fn finish_window_line(
    lines: &mut Vec<(usize, String)>,
    bytes: &mut Vec<u8>,
    line: usize,
    complete: bool,
) {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if complete {
        lines.push((line, String::from_utf8_lossy(bytes).into_owned()));
    }
    bytes.clear();
}

fn render_lines(
    lines: &[(usize, String)],
    total: usize,
    start: usize,
    limit: usize,
    truncated: bool,
) -> String {
    if total == 0 {
        return "[empty file]".to_string();
    }
    if start > total {
        return format!("[offset {start} is past the end; file has {total} line(s)]");
    }
    let end = start.saturating_add(limit).saturating_sub(1).min(total);
    let mut out = String::with_capacity(lines.iter().map(|(_, line)| line.len() + 8).sum());
    for (number, line) in lines {
        out.push_str(&format!("{number:>6}\t"));
        out.push_str(line);
        out.push('\n');
    }
    if start != 1 || end != total || truncated {
        out.push_str(&format!("\n[showed lines {start}-{end} of {total}"));
        if truncated {
            out.push_str(&format!(
                "; output capped at {MAX_BYTES} bytes — narrow the limit"
            ));
        } else if end < total {
            out.push_str(&format!("; call again with offset {}", end + 1));
        }
        out.push_str("]\n");
    }
    out
}

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        READ_FILE_TOOL
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file from the project. Call this whenever answering \
         depends on the actual contents of a file rather than on what its name \
         suggests. Paths are relative to the project root. Output is prefixed \
         with line numbers followed by a tab; those prefixes are display only \
         and must NOT be included in `edit_file` arguments. Reads up to 2000 \
         lines by default — use `offset` and `limit` to page through a larger \
         file. Likely-secret files (e.g. `.env`, private keys) require user \
         approval."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to the project root, e.g. src/main.rs"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line to start from. Defaults to the first line."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How many lines to return. Defaults to 2000."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        self.prepare_call(input)
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `path`".to_string())?;
        let offset = usize_field(&input, "offset")?;
        let limit = usize_field(&input, "limit")?;
        self.read_path(path, offset, limit)
            .await
            .map(super::ToolOutcome::text)
    }
}

/// Accept a positive integer field, rejecting anything that is present but not
/// usable rather than silently falling back to the default.
fn usize_field(input: &Value, name: &str) -> Result<Option<usize>, String> {
    match input.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| Some(n as usize))
            .ok_or_else(|| format!("`{name}` must be a positive integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::approval::AllowApprover;
    use crate::tools::ToolRegistry;
    use std::sync::Arc;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-read-file-{name}-"))
    }

    #[tokio::test]
    async fn reads_a_file_under_root() {
        let dir = scratch("ok");
        std::fs::write(dir.join("note.txt"), "hello").unwrap();
        let tool = ReadFile::new(&dir).unwrap();
        let out = tool.run(json!({ "path": "note.txt" })).await.unwrap().body;
        assert_eq!(out, "     1\thello\n");
    }

    #[tokio::test]
    async fn short_file_read_whole_has_no_range_footer() {
        let dir = scratch("no-footer");
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let tool = ReadFile::new(&dir).unwrap();
        let out = tool.run(json!({ "path": "a.txt" })).await.unwrap().body;
        assert_eq!(out, "     1\tone\n     2\ttwo\n");
    }

    #[tokio::test]
    async fn offset_and_limit_window_the_file() {
        let dir = scratch("window");
        let body: String = (1..=50).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.join("big.txt"), body).unwrap();
        let tool = ReadFile::new(&dir).unwrap();

        let out = tool
            .run(json!({ "path": "big.txt", "offset": 10, "limit": 3 }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("    10\tline10\n"), "{out}");
        assert!(out.contains("    12\tline12\n"), "{out}");
        assert!(!out.contains("line13"), "{out}");
        assert!(out.contains("showed lines 10-12 of 50"), "{out}");
        assert!(out.contains("offset 13"), "{out}");
    }

    #[tokio::test]
    async fn offset_past_end_reports_total_instead_of_empty() {
        let dir = scratch("past-end");
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let tool = ReadFile::new(&dir).unwrap();
        let out = tool
            .run(json!({ "path": "a.txt", "offset": 99 }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("past the end"), "{out}");
        assert!(out.contains("2 line(s)"), "{out}");
    }

    #[tokio::test]
    async fn rejects_non_positive_offset() {
        let dir = scratch("bad-offset");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let tool = ReadFile::new(&dir).unwrap();
        let err = tool
            .run(json!({ "path": "a.txt", "offset": 0 }))
            .await
            .unwrap_err();
        assert!(err.contains("positive integer"), "{err}");
    }

    #[tokio::test]
    async fn reads_beyond_the_old_64k_cap() {
        // The previous 64 KiB cap made this repo's own largest source file
        // unreadable. Anything under 256 KiB must now come back whole.
        let dir = scratch("large");
        let line = "x".repeat(99);
        let body: String = (0..1200).map(|_| format!("{line}\n")).collect();
        assert!(body.len() > 64 * 1024 && body.len() < MAX_BYTES);
        std::fs::write(dir.join("big.rs"), &body).unwrap();

        let tool = ReadFile::new(&dir).unwrap();
        let out = tool
            .run(json!({ "path": "big.rs", "limit": 5000 }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("  1200\t"), "last line must be present");
        assert!(!out.contains("truncated"), "{}", &out[..200.min(out.len())]);
    }

    #[tokio::test]
    async fn byte_truncation_is_announced_and_later_offsets_remain_reachable() {
        let dir = scratch("byte-cut");
        let line = "y".repeat(255);
        let body: String = (0..1200).map(|_| format!("{line}\n")).collect();
        assert!(body.len() > MAX_BYTES);
        std::fs::write(dir.join("huge.txt"), &body).unwrap();

        let tool = ReadFile::new(&dir).unwrap();
        let out = tool
            .run(json!({ "path": "huge.txt", "offset": 1000, "limit": 100 }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("  1000\t"), "late offset was not read: {out}");
        assert!(
            out.contains(&line),
            "complete selected lines should be kept"
        );
        assert!(
            !out.contains("capped at"),
            "small late window was capped: {out}"
        );
    }

    #[tokio::test]
    async fn a_large_first_window_drops_partial_lines_when_capped() {
        let dir = scratch("partial-line");
        let line = "y".repeat(255);
        let body: String = (0..1200).map(|_| format!("{line}\n")).collect();
        std::fs::write(dir.join("huge.txt"), body).unwrap();

        let tool = ReadFile::new(&dir).unwrap();
        let out = tool
            .run(json!({ "path": "huge.txt", "limit": 5000 }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("capped at"), "{out}");
        let last_content = out.lines().rfind(|line| line.contains('\t')).unwrap();
        assert!(
            last_content.ends_with(&line),
            "partial line leaked: {last_content}"
        );
    }

    #[tokio::test]
    async fn rejects_missing_path_and_escape() {
        let dir = scratch("bad");
        let tool = ReadFile::new(&dir).unwrap();

        let err = tool.run(json!({})).await.unwrap_err();
        assert!(err.contains("missing required field"), "{err}");

        let err = tool.run(json!({ "path": ".." })).await.unwrap_err();
        assert!(
            err.contains("outside the project root") || err.contains("cannot resolve"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn sensitive_read_requires_approval_risk() {
        let dir = scratch("secret");
        std::fs::write(dir.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(dir.join(".env.example"), "SECRET=\n").unwrap();
        let tool = ReadFile::new(&dir).unwrap();

        let prepared = tool.prepare(json!({ "path": ".env" })).unwrap();
        assert_eq!(prepared.risk, ToolRisk::Sensitive);
        assert!(prepared.risk.requires_approval());

        let example = tool.prepare(json!({ "path": ".env.example" })).unwrap();
        assert_eq!(example.risk, ToolRisk::Read);
        assert!(!example.risk.requires_approval());
    }

    #[tokio::test]
    async fn registry_executes_sensitive_after_prepare() {
        let dir = scratch("reg-secret");
        std::fs::write(dir.join(".env"), "SECRET=1\n").unwrap();
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ReadFile::new(&dir).unwrap()));
        let prepared = reg.prepare("read_file", json!({ "path": ".env" })).unwrap();
        assert_eq!(prepared.risk, ToolRisk::Sensitive);
        let _ = AllowApprover; // documents the approval path
        let out = reg.execute_prepared(prepared).await.unwrap().body;
        assert!(out.contains("SECRET=1"), "{out}");
    }
}
