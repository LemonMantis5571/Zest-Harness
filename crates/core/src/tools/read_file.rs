use std::path::Path;

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

/// Bytes this tool will read from a file, counted from byte zero.
///
/// Also the *reach* of an `offset`: the window is applied after the read, so no
/// offset addresses content past this point. Callers that hand the model a file
/// path larger than this must say so — see [`super::spill`].
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
        use tokio::io::AsyncReadExt;

        let resolved = self.root.resolve(path)?;
        let meta = tokio::fs::metadata(&resolved)
            .await
            .map_err(|e| format!("stat failed: {e}"))?;
        let file_len = meta.len() as usize;

        let file = tokio::fs::File::open(&resolved)
            .await
            .map_err(|e| format!("open failed: {e}"))?;
        // Bound before alloc: read at most MAX_BYTES (+1 to detect truncation).
        let mut buf = Vec::with_capacity(file_len.min(MAX_BYTES).saturating_add(1));
        let mut limited = file.take(MAX_BYTES as u64 + 1);
        limited
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        let byte_truncated = buf.len() > MAX_BYTES || file_len > MAX_BYTES;
        if buf.len() > MAX_BYTES {
            buf.truncate(MAX_BYTES);
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        Ok(number_lines(&text, offset, limit, byte_truncated, file_len))
    }
}

/// Render the requested window with `cat -n` style prefixes.
///
/// A byte-truncated tail is deliberately dropped rather than shown: the last
/// line would be cut mid-token, and a partial line that looks whole is exactly
/// what produces an `edit_file` call against text that does not exist.
fn number_lines(
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    byte_truncated: bool,
    file_len: usize,
) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    if byte_truncated && lines.len() > 1 {
        lines.pop();
    }
    let total = lines.len();

    // `offset` is 1-based to match what the model sees in the output.
    let start = offset.unwrap_or(1).max(1);
    let limit = limit.unwrap_or(DEFAULT_LINE_LIMIT).max(1);
    let start_index = start.saturating_sub(1);

    if total == 0 {
        return if byte_truncated {
            format!("[empty window; file is {file_len} bytes]")
        } else {
            "[empty file]".to_string()
        };
    }
    if start_index >= total {
        return format!(
            "[offset {start} is past the end; file has {total} line(s)\
             {}]",
            if byte_truncated {
                " in the first 256 KiB"
            } else {
                ""
            }
        );
    }

    let end_index = start_index.saturating_add(limit).min(total);
    let mut out = String::with_capacity((end_index - start_index) * 80);
    for (offset_in_slice, line) in lines[start_index..end_index].iter().enumerate() {
        out.push_str(&format!(
            "{:>6}\t{line}\n",
            start_index + offset_in_slice + 1
        ));
    }

    let shown_all_lines = start_index == 0 && end_index == total;
    if !shown_all_lines || byte_truncated {
        out.push_str(&format!(
            "\n[showed lines {}-{end_index} of {total}",
            start_index + 1
        ));
        if byte_truncated {
            out.push_str(&format!(
                "; file is {file_len} bytes and was cut at {MAX_BYTES} — lines beyond \
                 this point are not visible to any offset"
            ));
        } else if end_index < total {
            out.push_str(&format!("; call again with offset {}", end_index + 1));
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

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-read-file-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
    async fn byte_truncation_is_announced_and_drops_the_partial_line() {
        let dir = scratch("byte-cut");
        let line = "y".repeat(255);
        let body: String = (0..1200).map(|_| format!("{line}\n")).collect();
        assert!(body.len() > MAX_BYTES);
        std::fs::write(dir.join("huge.txt"), &body).unwrap();

        let tool = ReadFile::new(&dir).unwrap();
        let out = tool
            .run(json!({ "path": "huge.txt", "limit": 5000 }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("not visible to any offset"), "{out}");
        // The line the byte cap sliced through must not be presented as whole.
        let last_content = out.lines().rfind(|l| l.contains('\t')).unwrap();
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
