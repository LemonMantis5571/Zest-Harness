use std::io::Read;
use std::path::Path;

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde_json::{json, Value};

use super::approval::{ApprovalPreview, ToolRisk};
use super::prepared::PreparedToolCall;
use super::project::ProjectRoot;
use super::sensitive::is_sensitive_path;
use super::Tool;

const MAX_MATCHES: usize = 100;
/// Matched to `read_file`'s cap: a file the model can read whole must not have
/// its later half be silently unsearchable.
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_LINE_CHARS: usize = 400;

/// Search file contents under the project root.
pub struct Grep {
    root: ProjectRoot,
}

impl Grep {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }

    fn prepare_call(&self, input: Value) -> Result<PreparedToolCall, String> {
        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `pattern`".to_string())?;
        if pattern.is_empty() {
            return Err("pattern must not be empty".to_string());
        }
        // Validate regex before approval so bad patterns fail fast.
        let _ = Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;

        let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let start = self.root.resolve(path)?;
        let meta = std::fs::metadata(&start).map_err(|e| format!("stat failed: {e}"))?;

        // Direct-file grep on a sensitive path requires approval. Discovery
        // walks continue to omit sensitive files.
        if meta.is_file() {
            let rel = self.root.relativize(&start);
            if is_sensitive_path(&rel) {
                return Ok(PreparedToolCall::plain_with_preview(
                    "grep",
                    ToolRisk::Sensitive,
                    input,
                    ApprovalPreview {
                        path: rel.clone(),
                        summary: format!("Search sensitive file {rel}"),
                        diff: String::new(),
                    },
                ));
            }
        }

        Ok(PreparedToolCall::plain("grep", ToolRisk::Read, input))
    }

    async fn run_search(&self, input: Value) -> Result<String, String> {
        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `pattern`".to_string())?;
        if pattern.is_empty() {
            return Err("pattern must not be empty".to_string());
        }
        let re = Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;

        let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let start = self.root.resolve(path)?;

        let file_filter = match input.get("glob").and_then(Value::as_str) {
            Some(g) if !g.trim().is_empty() => Some(build_glob_filter(g.trim())?),
            _ => None,
        };

        // Bound: allow searching a sensitive file only when prepare marked it
        // Sensitive (approval already granted). Discovery still skips them.
        let allow_sensitive_file = {
            let meta = std::fs::metadata(&start).ok();
            meta.map(|m| m.is_file()).unwrap_or(false)
                && is_sensitive_path(&self.root.relativize(&start))
        };

        let root = self.root.clone();
        let matches = tokio::task::spawn_blocking(move || {
            search(
                &root,
                &start,
                &re,
                file_filter.as_ref(),
                allow_sensitive_file,
            )
        })
        .await
        .map_err(|e| format!("grep task failed: {e}"))??;

        format_results(matches)
    }
}

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents under the project root with a regular expression. \
         Optional `path` scopes to a file or directory; optional `glob` filters \
         by filename pattern (e.g. `*.rs`). Results are capped. Respects \
         `.gitignore` and skips `.git`, `.zest`, `target`, and `node_modules`. \
         Direct search of likely-secret files requires user approval. Prefer this \
         over shell search commands so paths, quoting, encoding, and output limits \
         are handled consistently across platforms."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory relative to the project root. Defaults to `.`."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional filename glob filter, e.g. *.rs or **/*.toml"
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        self.prepare_call(input)
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        self.run_search(input).await.map(super::ToolOutcome::text)
    }
}

fn build_glob_filter(pattern: &str) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern).map_err(|e| format!("invalid glob filter: {e}"))?);
    if !pattern.contains('/') && !pattern.contains('\\') && !pattern.starts_with("**/") {
        let anywhere = format!("**/{pattern}");
        builder.add(Glob::new(&anywhere).map_err(|e| format!("invalid glob filter: {e}"))?);
    }
    builder
        .build()
        .map_err(|e| format!("invalid glob filter: {e}"))
}

struct MatchLine {
    path: String,
    line_no: usize,
    line: String,
}

/// Search while walking — do not collect every file first.
fn search(
    root: &ProjectRoot,
    start: &Path,
    re: &Regex,
    file_filter: Option<&GlobSet>,
    allow_sensitive_file: bool,
) -> Result<Vec<MatchLine>, String> {
    let meta = std::fs::metadata(start).map_err(|e| format!("stat failed: {e}"))?;

    let mut matches = Vec::new();
    let mut output_bytes = 0usize;

    if meta.is_file() {
        let Ok(resolved) = root.confine(start) else {
            return Ok(matches);
        };
        let rel = root.relativize(&resolved);
        if is_sensitive_path(&rel) && !allow_sensitive_file {
            return Ok(matches);
        }
        search_file(
            root,
            &resolved,
            re,
            file_filter,
            &mut matches,
            &mut output_bytes,
        );
        return Ok(matches);
    }

    if !meta.is_dir() {
        return Err("path is neither a file nor a directory".to_string());
    }

    // Stream via ignore-aware walker; stop early when budgets are hit.
    let mut builder = ignore::WalkBuilder::new(start);
    super::walk::configure_walk_builder(&mut builder, root.as_path());

    for entry in builder.build().flatten() {
        if matches.len() >= MAX_MATCHES || output_bytes >= MAX_OUTPUT_BYTES {
            break;
        }
        if !super::walk::dir_entry_is_file(&entry) {
            continue;
        }
        let Ok(resolved) = root.confine(entry.path()) else {
            continue;
        };
        if !resolved.is_file() {
            continue;
        }
        let rel = root.relativize(&resolved);
        if is_sensitive_path(&rel) {
            continue;
        }
        search_file(
            root,
            &resolved,
            re,
            file_filter,
            &mut matches,
            &mut output_bytes,
        );
    }

    Ok(matches)
}

fn search_file(
    root: &ProjectRoot,
    file: &Path,
    re: &Regex,
    file_filter: Option<&GlobSet>,
    matches: &mut Vec<MatchLine>,
    output_bytes: &mut usize,
) {
    if matches.len() >= MAX_MATCHES || *output_bytes >= MAX_OUTPUT_BYTES {
        return;
    }

    let rel = root.relativize(file);
    if let Some(filter) = file_filter {
        if !filter.is_match(&rel) {
            return;
        }
    }

    let Ok(file) = std::fs::File::open(file) else {
        return;
    };
    let capacity = file
        .metadata()
        .map(|meta| meta.len().min(MAX_FILE_BYTES as u64) as usize)
        .unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    if file
        .take(MAX_FILE_BYTES as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return;
    }
    if bytes.contains(&0) {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);

    for (idx, line) in text.lines().enumerate() {
        if matches.len() >= MAX_MATCHES || *output_bytes >= MAX_OUTPUT_BYTES {
            break;
        }
        if re.is_match(line) {
            let line_no = idx + 1;
            let clipped = clip_chars(line, MAX_LINE_CHARS);
            *output_bytes = output_bytes
                .saturating_add(rel.len())
                .saturating_add(clipped.len())
                .saturating_add(16);
            matches.push(MatchLine {
                path: rel.clone(),
                line_no,
                line: clipped,
            });
        }
    }
}

fn clip_chars(s: &str, max_chars: usize) -> String {
    let mut iter = s.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        match iter.next() {
            Some(c) => out.push(c),
            None => return out,
        }
    }
    if iter.next().is_some() {
        out.push('…');
    }
    out
}

fn format_results(matches: Vec<MatchLine>) -> Result<String, String> {
    if matches.is_empty() {
        return Ok("(no matches)".to_string());
    }
    let truncated = matches.len() >= MAX_MATCHES;
    let mut out = String::new();
    for m in &matches {
        out.push_str(&format!("{}:{}:{}\n", m.path, m.line_no, m.line));
    }
    if truncated {
        out.push_str(&format!("\n[truncated at {MAX_MATCHES} matches]"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        let dir = crate::fsutil::ScratchDir::new(&format!("zest-grep-{name}-"));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "println is documented\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn finds_regex_matches() {
        let dir = scratch("basic");
        let tool = Grep::new(&dir).unwrap();
        let out = tool
            .run(json!({ "pattern": "println!", "glob": "*.rs" }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("src/main.rs:2:"), "{out}");
        assert!(!out.contains("README.md"), "{out}");
    }

    #[tokio::test]
    async fn scopes_to_a_single_file() {
        let dir = scratch("file");
        let tool = Grep::new(&dir).unwrap();
        let out = tool
            .run(json!({ "pattern": "helper", "path": "src/lib.rs" }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("src/lib.rs:1:"), "{out}");
        assert!(!out.contains("main.rs"), "{out}");
    }

    #[tokio::test]
    async fn searches_only_the_bounded_prefix_of_a_large_file() {
        let dir = scratch("bounded");
        let mut bytes = vec![b'\n'; MAX_FILE_BYTES - b"needle at limit\n".len()];
        bytes.extend_from_slice(b"needle at limit\nneedle past limit\n\0");
        std::fs::write(dir.join("large.txt"), bytes).unwrap();
        let tool = Grep::new(&dir).unwrap();
        let out = tool
            .run(json!({ "pattern": "needle", "path": "large.txt" }))
            .await
            .unwrap()
            .body;
        let line_no = MAX_FILE_BYTES - b"needle at limit\n".len() + 1;
        assert_eq!(out, format!("large.txt:{line_no}:needle at limit\n"));
    }

    #[tokio::test]
    async fn skips_binary_files_within_the_search_prefix() {
        let dir = scratch("binary");
        std::fs::write(dir.join("binary.txt"), b"needle\n\0").unwrap();
        let tool = Grep::new(&dir).unwrap();
        let out = tool
            .run(json!({ "pattern": "needle", "path": "binary.txt" }))
            .await
            .unwrap()
            .body;
        assert_eq!(out, "(no matches)");
    }

    #[tokio::test]
    async fn rejects_escape_and_bad_regex() {
        let dir = scratch("bad");
        let tool = Grep::new(&dir).unwrap();

        let err = tool.run(json!({ "pattern": "[" })).await.unwrap_err();
        assert!(err.contains("invalid regex"), "{err}");

        let err = tool
            .run(json!({ "pattern": "x", "path": ".." }))
            .await
            .unwrap_err();
        assert!(
            err.contains("outside the project root") || err.contains("cannot resolve"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn omits_sensitive_files_from_discovery() {
        let dir = scratch("secret");
        std::fs::write(dir.join(".env"), "SECRET_TOKEN=abc\n").unwrap();
        std::fs::write(dir.join("ok.txt"), "SECRET_TOKEN=visible\n").unwrap();
        let tool = Grep::new(&dir).unwrap();
        let out = tool
            .run(json!({ "pattern": "SECRET_TOKEN" }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("ok.txt"), "{out}");
        assert!(!out.contains(".env"), "{out}");
    }

    #[tokio::test]
    async fn direct_sensitive_file_requires_approval() {
        let dir = scratch("direct-secret");
        std::fs::write(dir.join(".env"), "SECRET_TOKEN=abc\n").unwrap();
        let tool = Grep::new(&dir).unwrap();
        let prepared = tool
            .prepare(json!({ "pattern": "SECRET", "path": ".env" }))
            .unwrap();
        assert_eq!(prepared.risk, ToolRisk::Sensitive);
        assert!(prepared.risk.requires_approval());
        let out = tool
            .run(json!({ "pattern": "SECRET", "path": ".env" }))
            .await
            .unwrap()
            .body;
        assert!(out.contains(".env"), "{out}");
    }

    #[test]
    fn clips_at_unicode_char_boundaries() {
        // Each emoji is one char but multiple bytes.
        let s = "😀".repeat(10);
        let clipped = clip_chars(&s, 3);
        assert_eq!(clipped, "😀😀😀…");

        // Mid-byte slice would panic; char clip must not.
        let mixed = "café😀xyz";
        let clipped = clip_chars(mixed, 5);
        assert_eq!(clipped, "café😀…");
    }

    #[tokio::test]
    async fn a_parent_gitignore_does_not_hide_project_files() {
        let outer = scratch("parent-ignore");
        std::fs::write(outer.join(".gitignore"), "*.txt\n").unwrap();
        let dir = outer.join("proj");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("hit.txt"), "needle here\n").unwrap();
        let tool = Grep::new(&dir).unwrap();
        let out = tool.run(json!({ "pattern": "needle" })).await.unwrap().body;
        assert!(out.contains("hit.txt"), "{out}");
    }

    #[tokio::test]
    async fn a_parent_ignore_file_does_not_hide_project_files() {
        let outer = scratch("parent-dot-ignore");
        std::fs::write(outer.join(".ignore"), "*.txt\n").unwrap();
        let dir = outer.join("proj");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("hit.txt"), "needle here\n").unwrap();
        let tool = Grep::new(&dir).unwrap();
        let out = tool.run(json!({ "pattern": "needle" })).await.unwrap().body;
        assert!(out.contains("hit.txt"), "{out}");
    }
}
