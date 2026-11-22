//! Targeted string replacement inside one project file.
//!
//! This exists for one reason: `write_file` charges the model the whole file in
//! *output* tokens to change three lines, and output is the slow, serialized,
//! expensive dimension of a turn. An edit that names only what changes costs a
//! rounding error by comparison.
//!
//! The replacement is computed at prepare time, so what reaches the approval
//! gate is a finished new body — identical in kind to a `write_file`. That lets
//! this tool reuse the whole write path: the BLAKE3 pre-image, the diff
//! preview, the desktop approval card, and the atomic replace, without adding
//! any new plumbing.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::approval::{ApprovalPreview, ToolRisk};
use super::edit_match::{closest_lines, locate, splice, Located, MatchKind};
use super::prepared::{PreImage, PreparedToolCall};
use super::project::ProjectRoot;
use super::sensitive::is_sensitive_path;
use super::write_file::{bounded_unified_diff, commit_prepared_write};
use super::Tool;

const MAX_BYTES: usize = 1024 * 1024;

pub struct EditFile {
    root: ProjectRoot,
}

impl EditFile {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }

    pub fn prepare_call(&self, input: Value) -> Result<PreparedToolCall, String> {
        let path = str_field(&input, "path")?;
        let old_string = str_field(&input, "old_string")?;
        let new_string = match input.get("new_string") {
            Some(Value::String(s)) => s.as_str(),
            // Deleting text is a legitimate edit, so an empty replacement is
            // allowed — but it has to be spelled out rather than omitted.
            Some(other) => return Err(format!("`new_string` must be a string, got {other}")),
            None => return Err("missing required field `new_string`".into()),
        };

        if old_string.is_empty() {
            return Err("`old_string` must not be empty; use write_file to create a file".into());
        }
        if old_string == new_string {
            return Err("`old_string` and `new_string` are identical; nothing to change".into());
        }

        let replace_all = match input.get("replace_all") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(other) => return Err(format!("`replace_all` must be a boolean, got {other}")),
        };

        let resolved = self.root.resolve_for_write(path)?;
        let rel = self.root.relativize(&resolved);

        let bytes = match std::fs::metadata(&resolved) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "{rel} does not exist; edit_file only modifies existing files — use write_file to create one"
                ));
            }
            Err(e) => return Err(format!("cannot stat {rel}: {e}")),
            Ok(meta) if meta.is_dir() => return Err(format!("{rel} is a directory, not a file")),
            Ok(_) => std::fs::read(&resolved).map_err(|e| format!("cannot read {rel}: {e}"))?,
        };
        let old_text = String::from_utf8(bytes)
            .map_err(|_| format!("{rel} is not valid UTF-8; refusing to edit it as text"))?;

        let (ranges, kind) = match locate(&old_text, old_string, replace_all) {
            Located::Found { ranges, kind } => (ranges, kind),
            Located::Ambiguous {
                count,
                kind: MatchKind::Exact,
            } => {
                return Err(format!(
                    "`old_string` appears {count} times in {rel}. Include more surrounding \
                     context so it matches exactly once, or set replace_all to true."
                ))
            }
            Located::Ambiguous { count, kind } => {
                return Err(format!(
                    "`old_string` was not found exactly in {rel}; ignoring {} it matches \
                     {count} places. Quote the text exactly as read_file shows it, with \
                     enough surrounding context to match once.",
                    kind.ignored()
                ))
            }
            Located::Missing => return Err(self.miss_message(path, &rel, &old_text, old_string)),
        };
        let matches = ranges.len();

        let new_text = splice(&old_text, &ranges, new_string);
        if new_text == old_text {
            return Err(format!(
                "this edit would leave {rel} unchanged; `new_string` must differ from the \
                 text it replaces"
            ));
        }
        if new_text.len() > MAX_BYTES {
            return Err(format!(
                "result would be {} bytes; max is {MAX_BYTES}",
                new_text.len()
            ));
        }

        let preimage = PreImage::of_bytes(old_text.as_bytes());
        let diff = bounded_unified_diff(&rel, &old_text, &new_text, true);
        let summary = if matches > 1 {
            format!("Edit {rel} ({matches} occurrences)")
        } else if kind != MatchKind::Exact {
            format!("Edit {rel} (matched ignoring {})", kind.ignored())
        } else {
            format!("Edit {rel}")
        };

        Ok(PreparedToolCall::write_kind(
            "edit_file",
            resolved,
            rel.clone(),
            new_text,
            preimage,
            ApprovalPreview {
                path: rel,
                summary,
                diff,
            },
        ))
    }

    /// The error for a quote that matched nothing, with the closest lines of
    /// the file when the model could have read them anyway: never for a path
    /// `read_file` would refuse or would put behind a sensitive-file approval.
    fn miss_message(&self, path: &str, rel: &str, text: &str, old_string: &str) -> String {
        let mut message = format!(
            "`old_string` was not found in {rel}. Read the file and quote the text \
             exactly, without the line-number prefixes read_file adds."
        );
        if is_sensitive_path(rel) || self.root.resolve(path).is_err() {
            return message;
        }
        if let Some(lines) = closest_lines(text, old_string) {
            message.push_str(&format!(
                "\nThe closest text in {rel} is (line numbers are not part of the file):\n{lines}"
            ));
        }
        message
    }
}

fn str_field<'a>(input: &'a Value, name: &str) -> Result<&'a str, String> {
    input
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing required field `{name}`"))
}

#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace an exact string in an existing project file. Prefer this over \
         write_file for any change to a file that already exists — it is far \
         cheaper and cannot clobber the parts you did not touch. Read the file \
         first: quote `old_string` exactly as read_file shows it, including \
         indentation, and do NOT include the line-number prefixes read_file \
         adds. Line endings do not matter: the file keeps its own. `old_string` \
         must appear exactly once unless replace_all is set. Requires user \
         approval before the write runs."
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
                "old_string": {
                    "type": "string",
                    "description": "Exact text to replace, including indentation. Must be unique in the file unless replace_all is true."
                },
                "new_string": {
                    "type": "string",
                    "description": "Text to replace it with. Empty string deletes the matched text."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence instead of requiring a unique match. Defaults to false."
                }
            },
            "required": ["path", "old_string", "new_string"],
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
        tokio::task::spawn_blocking(move || commit_prepared_write(&root, prepared))
            .await
            .map_err(|e| format!("edit task failed: {e}"))?
            .map(|w| super::ToolOutcome::text(format!("edited {} ({} bytes)", w.path, w.bytes)))
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let prepared = self.prepare_call(input)?;
        self.execute_prepared(prepared).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_a_secret_file_is_sensitive() {
        let dir = scratch("secret");
        std::fs::write(
            dir.join(".env"),
            "TOKEN=old
",
        )
        .unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let prepared = tool
            .prepare_call(json!({ "path": ".env", "old_string": "old", "new_string": "new" }))
            .unwrap();
        assert_eq!(prepared.risk, crate::tools::approval::ToolRisk::Sensitive);
    }

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-edit-file-{name}-"))
    }

    #[tokio::test]
    async fn replaces_a_unique_match() {
        let dir = scratch("unique");
        std::fs::write(dir.join("a.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let out = tool
            .run(json!({
                "path": "a.rs",
                "old_string": "let x = 1;",
                "new_string": "let x = 2;"
            }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("edited a.rs"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.rs")).unwrap(),
            "fn main() {\n    let x = 2;\n}\n"
        );
    }

    #[tokio::test]
    async fn ambiguous_match_is_rejected_with_the_count() {
        let dir = scratch("ambiguous");
        std::fs::write(dir.join("a.rs"), "let x = 1;\nlet x = 1;\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let err = tool
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "let x = 1;",
                "new_string": "let x = 2;"
            }))
            .unwrap_err();
        assert!(err.contains("appears 2 times"), "{err}");
        assert!(err.contains("replace_all"), "{err}");
        // Nothing may have been written on the rejected path.
        assert_eq!(
            std::fs::read_to_string(dir.join("a.rs")).unwrap(),
            "let x = 1;\nlet x = 1;\n"
        );
    }

    #[tokio::test]
    async fn replace_all_takes_every_occurrence() {
        let dir = scratch("all");
        std::fs::write(dir.join("a.rs"), "a\nb\na\nb\na\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        tool.run(json!({
            "path": "a.rs",
            "old_string": "a",
            "new_string": "c",
            "replace_all": true
        }))
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.rs")).unwrap(),
            "c\nb\nc\nb\nc\n"
        );
    }

    #[tokio::test]
    async fn missing_match_names_the_line_prefix_trap() {
        let dir = scratch("nomatch");
        std::fs::write(dir.join("a.rs"), "let x = 1;\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let err = tool
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "     1\tlet x = 1;",
                "new_string": "let x = 2;"
            }))
            .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("line-number prefixes"), "{err}");
    }

    #[tokio::test]
    async fn refuses_to_create_a_missing_file() {
        let dir = scratch("missing");
        let tool = EditFile::new(&dir).unwrap();
        let err = tool
            .prepare_call(json!({
                "path": "nope.rs",
                "old_string": "x",
                "new_string": "y"
            }))
            .unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        assert!(err.contains("write_file"), "{err}");
    }

    #[tokio::test]
    async fn rejects_a_no_op_edit() {
        let dir = scratch("noop");
        std::fs::write(dir.join("a.rs"), "same\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let err = tool
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "same",
                "new_string": "same"
            }))
            .unwrap_err();
        assert!(err.contains("identical"), "{err}");
    }

    #[tokio::test]
    async fn empty_new_string_deletes_the_match() {
        let dir = scratch("delete");
        std::fs::write(dir.join("a.rs"), "keep\nDROP ME\nkeep\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        tool.run(json!({
            "path": "a.rs",
            "old_string": "DROP ME\n",
            "new_string": ""
        }))
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.rs")).unwrap(),
            "keep\nkeep\n"
        );
    }

    #[tokio::test]
    async fn a_second_prepared_edit_needs_a_fresh_prepare_after_the_first_write() {
        let dir = scratch("batch-stale");
        std::fs::write(dir.join("a.rs"), "alpha\nbeta\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let first = tool
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "alpha",
                "new_string": "ALPHA"
            }))
            .unwrap();
        let second = tool
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "beta",
                "new_string": "BETA"
            }))
            .unwrap();
        tool.execute_prepared(first).await.unwrap();
        let err = tool.execute_prepared(second).await.unwrap_err();
        assert!(err.contains("changed after approval"), "{err}");
        let fresh = tool
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "beta",
                "new_string": "BETA"
            }))
            .unwrap();
        tool.execute_prepared(fresh).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.rs")).unwrap(),
            "ALPHA\nBETA\n"
        );
    }

    #[tokio::test]
    async fn stale_preimage_aborts_the_edit() {
        let dir = scratch("stale");
        std::fs::write(dir.join("a.rs"), "v1\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let prepared = tool
            .prepare_call(json!({ "path": "a.rs", "old_string": "v1", "new_string": "v2" }))
            .unwrap();
        std::fs::write(dir.join("a.rs"), "changed underneath\n").unwrap();
        let err = tool.execute_prepared(prepared).await.unwrap_err();
        assert!(err.contains("changed after approval"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.rs")).unwrap(),
            "changed underneath\n"
        );
    }

    #[tokio::test]
    async fn crlf_and_multibyte_survive_untouched() {
        let dir = scratch("crlf");
        std::fs::write(dir.join("a.txt"), "héllo\r\nwörld — ok\r\ntail\r\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        tool.run(json!({
            "path": "a.txt",
            "old_string": "wörld — ok",
            "new_string": "wörld — better"
        }))
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "héllo\r\nwörld — better\r\ntail\r\n"
        );
    }

    fn prepared_body(prepared: &PreparedToolCall) -> &str {
        match &prepared.kind {
            crate::tools::prepared::PreparedKind::WriteFile { content, .. } => content,
            _ => panic!("edit_file must prepare a write"),
        }
    }

    /// Prepare, check the approved body is what lands on disk, and return it.
    async fn edit(dir: &Path, input: Value) -> (PreparedToolCall, String) {
        let tool = EditFile::new(dir).unwrap();
        let prepared = tool.prepare_call(input).unwrap();
        let approved = prepared_body(&prepared).to_string();
        let path = prepared.preview.path.clone();
        tool.execute_prepared(prepared.clone()).await.unwrap();
        let written = std::fs::read_to_string(dir.join(&path)).unwrap();
        assert_eq!(
            written, approved,
            "disk must hold exactly the approved bytes"
        );
        (prepared, written)
    }

    #[tokio::test]
    async fn multi_line_edit_matches_a_crlf_file_and_keeps_crlf() {
        let dir = scratch("crlf-multi");
        std::fs::write(
            dir.join("a.rs"),
            "fn main() {\r\n    let x = 1;\r\n    let y = 2;\r\n}\r\n",
        )
        .unwrap();
        // What a model quotes after read_file: LF only.
        let (prepared, written) = edit(
            &dir,
            json!({
                "path": "a.rs",
                "old_string": "    let x = 1;\n    let y = 2;\n",
                "new_string": "    let x = 10;\n    let y = 20;\n    let z = 30;\n"
            }),
        )
        .await;
        assert_eq!(
            written,
            "fn main() {\r\n    let x = 10;\r\n    let y = 20;\r\n    let z = 30;\r\n}\r\n"
        );
        assert_eq!(prepared.preview.summary, "Edit a.rs");
        assert!(
            prepared.preview.diff.contains("+    let z = 30;"),
            "{}",
            prepared.preview.diff
        );
    }

    #[tokio::test]
    async fn mixed_line_endings_stay_as_they_were() {
        let dir = scratch("mixed");
        std::fs::write(
            dir.join("a.txt"),
            "lf one\nlf two\ncr one\r\ncr two\r\ntail\n",
        )
        .unwrap();
        let (_, written) = edit(
            &dir,
            json!({
                "path": "a.txt",
                "old_string": "cr one\ncr two",
                "new_string": "cr ONE\ncr TWO\ncr THREE"
            }),
        )
        .await;
        assert_eq!(
            written,
            "lf one\nlf two\ncr ONE\r\ncr TWO\r\ncr THREE\r\ntail\n"
        );
        let (_, written) = edit(
            &dir,
            json!({
                "path": "a.txt",
                "old_string": "lf two",
                "new_string": "lf two\nlf extra"
            }),
        )
        .await;
        assert_eq!(
            written,
            "lf one\nlf two\nlf extra\ncr ONE\r\ncr TWO\r\ncr THREE\r\ntail\n"
        );
    }

    #[tokio::test]
    async fn a_line_ending_only_change_is_refused_as_a_no_op() {
        let dir = scratch("crlf-noop");
        std::fs::write(dir.join("a.txt"), "a\r\nb\r\n").unwrap();
        let err = EditFile::new(&dir)
            .unwrap()
            .prepare_call(json!({ "path": "a.txt", "old_string": "a\r\n", "new_string": "a\n" }))
            .unwrap_err();
        assert!(err.contains("unchanged"), "{err}");
    }

    #[tokio::test]
    async fn punctuation_fallback_ignores_quotes_dashes_and_nbsp() {
        let dir = scratch("punct");
        std::fs::write(
            dir.join("a.md"),
            "say \u{201C}hi\u{201D} \u{2014} it\u{2019}s 5\u{00A0}km\nother\n",
        )
        .unwrap();
        let (prepared, written) = edit(
            &dir,
            json!({
                "path": "a.md",
                "old_string": "say \"hi\" - it's 5 km",
                "new_string": "say \"bye\" - it's 6 km"
            }),
        )
        .await;
        assert_eq!(written, "say \"bye\" - it's 6 km\nother\n");
        assert!(
            prepared
                .preview
                .summary
                .contains("matched ignoring curly quotes"),
            "{}",
            prepared.preview.summary
        );
    }

    #[tokio::test]
    async fn trailing_whitespace_fallback_matches_lines_with_trailing_spaces() {
        let dir = scratch("trailing");
        std::fs::write(
            dir.join("a.py"),
            "def f():  \r\n    return 1\t\r\n\r\nx = 2\r\n",
        )
        .unwrap();
        let (prepared, written) = edit(
            &dir,
            json!({
                "path": "a.py",
                "old_string": "def f():\n    return 1",
                "new_string": "def f():\n    return 2"
            }),
        )
        .await;
        assert_eq!(written, "def f():\r\n    return 2\t\r\n\r\nx = 2\r\n");
        assert!(
            prepared.preview.summary.contains("trailing whitespace"),
            "{}",
            prepared.preview.summary
        );
    }

    #[tokio::test]
    async fn an_exact_match_wins_over_a_loose_one() {
        let dir = scratch("exact-first");
        std::fs::write(dir.join("a.txt"), "it\u{2019}s\nit's\n").unwrap();
        let (_, written) = edit(
            &dir,
            json!({ "path": "a.txt", "old_string": "it's", "new_string": "it is" }),
        )
        .await;
        assert_eq!(written, "it\u{2019}s\nit is\n");
    }

    #[tokio::test]
    async fn an_ambiguous_loose_match_is_refused() {
        let dir = scratch("loose-ambiguous");
        let body = "a \u{2014} b\na \u{2013} b\n";
        std::fs::write(dir.join("a.txt"), body).unwrap();
        for replace_all in [false, true] {
            let err = EditFile::new(&dir)
                .unwrap()
                .prepare_call(json!({
                    "path": "a.txt",
                    "old_string": "a - b",
                    "new_string": "c",
                    "replace_all": replace_all
                }))
                .unwrap_err();
            assert!(err.contains("matches 2 places"), "{err}");
            assert!(err.contains("ignoring curly quotes"), "{err}");
        }
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), body);
    }

    #[tokio::test]
    async fn a_miss_shows_the_closest_lines() {
        let dir = scratch("miss-hint");
        std::fs::write(
            dir.join("a.rs"),
            "use std::io;\n\nfn total(items: &[u32]) -> u32 {\n    items.iter().sum()\n}\n",
        )
        .unwrap();
        let err = EditFile::new(&dir)
            .unwrap()
            .prepare_call(json!({
                "path": "a.rs",
                "old_string": "fn total(items: &[u64]) -> u64 {\n    items.iter().sum()",
                "new_string": "fn total(items: &[u64]) -> u64 {\n    0"
            }))
            .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("closest text in a.rs"), "{err}");
        assert!(
            err.contains("     3\tfn total(items: &[u32]) -> u32 {"),
            "{err}"
        );
        assert!(err.contains("     4\t    items.iter().sum()"), "{err}");
    }

    #[tokio::test]
    async fn a_miss_on_a_sensitive_file_shows_no_lines() {
        let dir = scratch("miss-secret");
        std::fs::write(dir.join(".env"), "API_TOKEN=sk-live-123456\n").unwrap();
        let err = EditFile::new(&dir)
            .unwrap()
            .prepare_call(json!({
                "path": ".env",
                "old_string": "API_TOKEN=sk-live-123457",
                "new_string": "API_TOKEN=x"
            }))
            .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(!err.contains("sk-live"), "{err}");
    }

    #[tokio::test]
    async fn prepare_carries_write_risk_and_a_real_diff() {
        let dir = scratch("preview");
        std::fs::write(dir.join("a.rs"), "old\nkeep\n").unwrap();
        let tool = EditFile::new(&dir).unwrap();
        let prepared = tool
            .prepare_call(json!({ "path": "a.rs", "old_string": "old", "new_string": "new" }))
            .unwrap();
        assert_eq!(prepared.tool_name, "edit_file");
        assert_eq!(prepared.risk, ToolRisk::Write);
        assert!(prepared.risk.requires_approval());
        assert_eq!(prepared.preview.path, "a.rs");
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
    }

    #[tokio::test]
    async fn rejects_escape_and_missing_fields() {
        let dir = scratch("bad");
        let tool = EditFile::new(&dir).unwrap();
        let err = tool
            .prepare_call(json!({ "path": "a.rs", "old_string": "x" }))
            .unwrap_err();
        assert!(err.contains("new_string"), "{err}");
        let err = tool
            .prepare_call(json!({ "path": "../x.txt", "old_string": "x", "new_string": "y" }))
            .unwrap_err();
        assert!(err.contains("escapes") || err.contains("outside"), "{err}");
    }
}
