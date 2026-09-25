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

/// Maximum rendered bytes (line-number prefixes included) for one requested
/// window. Any line offset remains reachable; this only bounds one page.
pub const MAX_BYTES: usize = 256 * 1024;
/// Lines returned when the call does not ask for a narrower window.
const DEFAULT_LINE_LIMIT: usize = 2_000;
/// How far past the window the reader keeps counting lines for the footer's
/// total. A page of a multi-gigabyte log must not cost a read of all of it.
const MAX_COUNT_BYTES_AFTER_WINDOW: u64 = 64 * 1024 * 1024;

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
            let window =
                collect_lines_window(&resolved, offset, limit, MAX_COUNT_BYTES_AFTER_WINDOW)?;
            Ok(render_lines(&window, offset))
        })
        .await
        .map_err(|e| format!("read worker failed: {e}"))?
    }
}

/// The requested lines, plus what the footer needs to describe them honestly.
struct LineWindow {
    lines: Vec<(usize, String)>,
    /// Lines counted. Exact when `total_known`, otherwise a lower bound.
    total_lines: usize,
    /// False when counting stopped `MAX_COUNT_BYTES_AFTER_WINDOW` past the
    /// window with more of the file unread.
    total_known: bool,
    /// The byte budget ended the window before `limit` lines.
    capped: bool,
    /// A single line too long for the budget, shown as a prefix:
    /// `(line number, length in bytes, bytes shown, whether the length is the
    /// whole line or only as far as the reader went)`.
    clipped_line: Option<(usize, usize, usize, bool)>,
}

/// Rendered cost of one line: `{number:>6}\t`, the content, and its newline.
fn rendered_len(number: usize, content: usize) -> usize {
    let digits = number.checked_ilog10().map_or(1, |d| d as usize + 1);
    digits.max(6) + 1 + content + 1
}

/// Stream the file, retaining only the requested window within `MAX_BYTES` of
/// rendered output, and keep counting lines up to `count_budget` bytes past
/// it. Memory stays bounded by the budget whatever the file or line size.
fn collect_lines_window(
    path: &PathBuf,
    start: usize,
    limit: usize,
    count_budget: u64,
) -> Result<LineWindow, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut reader = BufReader::new(file);
    let end = start.saturating_add(limit);
    let mut window = LineWindow {
        lines: Vec::new(),
        total_lines: 0,
        total_known: true,
        capped: false,
        clipped_line: None,
    };
    let mut line_number = 1usize;
    let mut line_len = 0usize;
    let mut line = Vec::new();
    let mut rendered = 0usize;
    let mut counted_after_window = 0u64;

    loop {
        let collecting = !window.capped && (start..end).contains(&line_number);
        let buffer = reader.fill_buf().map_err(|e| format!("read failed: {e}"))?;
        if buffer.is_empty() {
            if line_len > 0 {
                if collecting {
                    finish_window_line(
                        &mut window,
                        &mut line,
                        line_number,
                        line_len,
                        &mut rendered,
                    );
                }
                line_number = line_number.saturating_add(1);
            }
            break;
        }
        // Past the budget, stop: either the window is done, or it is one line
        // longer than a page that would otherwise be read to its end.
        let line_overflowed = collecting && line.len() > MAX_BYTES;
        if (!collecting || line_overflowed)
            && line_number >= start
            && counted_after_window > count_budget
        {
            if line_overflowed {
                finish_window_line(&mut window, &mut line, line_number, line_len, &mut rendered);
                if let Some(clipped) = window.clipped_line.as_mut() {
                    clipped.3 = false;
                }
            }
            window.total_known = false;
            break;
        }
        let (consumed, newline) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(position) => (position + 1, true),
            None => (buffer.len(), false),
        };
        let content = &buffer[..consumed - usize::from(newline)];
        if collecting && !line_overflowed {
            // One byte past the budget is enough to know a line does not fit.
            let room = (MAX_BYTES + 1).saturating_sub(line.len());
            line.extend_from_slice(&content[..room.min(content.len())]);
        } else if line_number >= start {
            counted_after_window = counted_after_window.saturating_add(consumed as u64);
        }
        line_len = line_len.saturating_add(content.len());
        reader.consume(consumed);

        if newline {
            if collecting {
                finish_window_line(&mut window, &mut line, line_number, line_len, &mut rendered);
            }
            line_number = line_number.saturating_add(1);
            line_len = 0;
        }
    }

    window.total_lines = line_number.saturating_sub(1);
    Ok(window)
}

fn finish_window_line(
    window: &mut LineWindow,
    bytes: &mut Vec<u8>,
    number: usize,
    full_len: usize,
    rendered: &mut usize,
) {
    let complete = bytes.len() == full_len;
    if complete && bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let cost = rendered_len(number, bytes.len());
    if complete && rendered.saturating_add(cost) <= MAX_BYTES {
        window
            .lines
            .push((number, String::from_utf8_lossy(bytes).into_owned()));
        *rendered += cost;
    } else if window.lines.is_empty() {
        // A first line that alone exceeds the budget is shown as a prefix rather
        // than dropped, or no call could ever return any of it.
        let mut cut = MAX_BYTES
            .saturating_sub(rendered_len(number, 0))
            .min(bytes.len());
        while cut > 0 && cut < bytes.len() && (bytes[cut] & 0xC0) == 0x80 {
            cut -= 1;
        }
        window
            .lines
            .push((number, String::from_utf8_lossy(&bytes[..cut]).into_owned()));
        let length = if complete { bytes.len() } else { full_len };
        window.clipped_line = Some((number, length, cut, true));
        window.capped = true;
    } else {
        window.capped = true;
    }
    bytes.clear();
}

fn render_lines(window: &LineWindow, start: usize) -> String {
    let total = window.total_lines;
    if window.total_known && total == 0 {
        return "[empty file]".to_string();
    }
    if window.total_known && start > total {
        return format!("[offset {start} is past the end; file has {total} line(s)]");
    }
    let shown_end = window
        .lines
        .last()
        .map_or(start.saturating_sub(1), |(number, _)| *number);
    let has_more = !window.total_known || shown_end < total;
    let mut out = String::with_capacity(
        window
            .lines
            .iter()
            .map(|(number, line)| rendered_len(*number, line.len()))
            .sum(),
    );
    for (number, line) in &window.lines {
        out.push_str(&format!("{number:>6}\t"));
        out.push_str(line);
        out.push('\n');
    }
    if start != 1 || has_more || window.capped {
        let total_text = if window.total_known {
            total.to_string()
        } else {
            format!("more than {total} (stopped counting)")
        };
        out.push_str(&format!(
            "\n[showed lines {start}-{shown_end} of {total_text}"
        ));
        if let Some((number, length, shown, whole)) = window.clipped_line {
            let length = if whole {
                format!("{length} bytes")
            } else {
                format!("more than {length} bytes")
            };
            out.push_str(&format!(
                "; line {number} is {length} and only its first {shown} are shown — \
                 use grep to find text further into it"
            ));
        } else if window.capped {
            out.push_str(&format!("; output capped at {MAX_BYTES} bytes"));
        }
        if has_more {
            out.push_str(&format!("; call again with offset {}", shown_end + 1));
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
        // unreadable. A file whose numbered lines fit in 256 KiB comes back
        // whole when the limit allows every line.
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
        assert!(
            !out.contains("[showed lines"),
            "{}",
            &out[out.len() - 200..]
        );
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
        // The footer must name the last line actually shown and how to go on,
        // or a model that trusts it skips the lines it never saw.
        let last_shown: usize = last_content
            .split('\t')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(last_shown < 1200, "{last_shown}");
        assert!(
            out.contains(&format!("[showed lines 1-{last_shown} of 1200;")),
            "{}",
            &out[out.len() - 200..]
        );
        assert!(
            out.contains(&format!("call again with offset {}", last_shown + 1)),
            "{}",
            &out[out.len() - 200..]
        );
        assert!(out.len() <= MAX_BYTES + 200, "{}", out.len());
    }

    async fn read(dir: &crate::fsutil::ScratchDir, input: Value) -> String {
        ReadFile::new(dir).unwrap().run(input).await.unwrap().body
    }

    #[tokio::test]
    async fn a_line_longer_than_the_budget_is_shown_as_a_prefix() {
        let dir = scratch("long-line");
        let long = "z".repeat(MAX_BYTES + 50_000);
        std::fs::write(dir.join("one.json"), &long).unwrap();
        let out = read(&dir, json!({ "path": "one.json" })).await;
        assert!(out.starts_with("     1\tzzzz"), "{}", &out[..40]);
        assert!(out.len() <= MAX_BYTES + 300, "{}", out.len());
        assert!(
            out.contains(&format!("line 1 is {} bytes", long.len())),
            "{}",
            &out[out.len() - 300..]
        );
        assert!(!out.contains("call again"), "{}", &out[out.len() - 300..]);

        std::fs::write(dir.join("three.txt"), format!("a\n{long}\nc\n")).unwrap();
        let out = read(
            &dir,
            json!({ "path": "three.txt", "offset": 2, "limit": 1 }),
        )
        .await;
        assert!(out.starts_with("     2\tzzzz"), "{}", &out[..40]);
        assert!(out.contains("line 2 is"), "{}", &out[out.len() - 300..]);
        assert!(
            out.contains("call again with offset 3"),
            "{}",
            &out[out.len() - 300..]
        );
    }

    #[tokio::test]
    async fn blank_lines_count_against_the_budget_and_leave_no_gaps() {
        let dir = scratch("blank-lines");
        let first = "A".repeat(MAX_BYTES - rendered_len(1, 0) - rendered_len(2, 0));
        std::fs::write(dir.join("gaps.txt"), format!("{first}\n\nB\n\nC\n")).unwrap();
        let out = read(&dir, json!({ "path": "gaps.txt" })).await;
        assert!(out.contains("     2\t\n"), "{}", &out[out.len() - 200..]);
        assert!(!out.contains("     3\t"), "{}", &out[out.len() - 200..]);
        assert!(
            !out.contains("     4\t"),
            "a blank line after the cap was shown"
        );
        assert!(
            out.contains("[showed lines 1-2 of 5; output capped"),
            "{}",
            &out[out.len() - 200..]
        );
        assert!(out.contains("offset 3"), "{}", &out[out.len() - 200..]);

        std::fs::write(dir.join("empty-lines.txt"), "\n".repeat(200_000)).unwrap();
        let out = read(
            &dir,
            json!({ "path": "empty-lines.txt", "limit": 1_000_000 }),
        )
        .await;
        assert!(out.len() <= MAX_BYTES + 200, "{}", out.len());
        assert!(out.contains("output capped"), "{}", &out[out.len() - 200..]);
    }

    #[tokio::test]
    async fn crlf_empty_unterminated_and_split_utf8_files_read_cleanly() {
        let dir = scratch("edge-files");
        std::fs::write(dir.join("crlf.txt"), "one\r\ntwo\r\n").unwrap();
        assert_eq!(
            read(&dir, json!({ "path": "crlf.txt" })).await,
            "     1\tone\n     2\ttwo\n"
        );

        std::fs::write(dir.join("empty.txt"), "").unwrap();
        assert_eq!(
            read(&dir, json!({ "path": "empty.txt" })).await,
            "[empty file]"
        );

        std::fs::write(dir.join("tail.txt"), "a\nb\nc").unwrap();
        let out = read(&dir, json!({ "path": "tail.txt", "offset": 3 })).await;
        assert!(out.starts_with("     3\tc\n"), "{out}");
        assert!(out.contains("[showed lines 3-3 of 3]"), "{out}");

        // A two-byte character straddling the reader's 8 KiB buffer boundary.
        let text = format!("{}é tail\n", "x".repeat(8191));
        std::fs::write(dir.join("split.txt"), &text).unwrap();
        let out = read(&dir, json!({ "path": "split.txt" })).await;
        assert!(out.contains("é tail"), "{}", &out[out.len() - 40..]);
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn counting_stops_past_the_window_on_huge_files() {
        let dir = scratch("count-budget");
        let body: String = (1..=1000).map(|i| format!("line{i}\n")).collect();
        let path = dir.join("big.log");
        std::fs::write(&path, body).unwrap();

        let window = collect_lines_window(&path, 1, 10, 100).unwrap();
        assert!(!window.total_known);
        let out = render_lines(&window, 1);
        assert!(out.contains("    10\tline10\n"), "{out}");
        assert!(out.contains("of more than"), "{out}");
        assert!(out.contains("call again with offset 11"), "{out}");

        let window = collect_lines_window(&path, 1, 10, u64::MAX).unwrap();
        assert!(window.total_known);
        assert!(render_lines(&window, 1).contains("[showed lines 1-10 of 1000;"));

        // One endless line inside the window stops at the same budget.
        let path = dir.join("one-line.min.js");
        std::fs::write(&path, "q".repeat(MAX_BYTES + 50_000)).unwrap();
        let window = collect_lines_window(&path, 1, 10, 1_000).unwrap();
        assert!(!window.total_known);
        let out = render_lines(&window, 1);
        assert!(
            out.contains("line 1 is more than"),
            "{}",
            &out[out.len() - 200..]
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
