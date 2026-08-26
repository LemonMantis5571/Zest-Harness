use std::path::Path;

use async_trait::async_trait;
use globset::{Glob, GlobSetBuilder};
use serde_json::{json, Value};

use super::project::ProjectRoot;
use super::walk::walk_files;
use super::Tool;

const MAX_MATCHES: usize = 200;

/// Find files by glob pattern, confined to a project root.
pub struct GlobFiles {
    root: ProjectRoot,
}

impl GlobFiles {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }
}

#[async_trait]
impl Tool for GlobFiles {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find file paths under the project root matching a glob pattern \
         (e.g. `**/*.rs`, `src/**/*.toml`). Returns paths relative to the \
         project root, sorted. Respects `.gitignore` and skips `.git`, \
         `.zest`, `target`, and `node_modules`. Does not read file contents."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern relative to the project root, e.g. **/*.rs"
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required field `pattern`".to_string())?;
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Err("pattern must not be empty".to_string());
        }

        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new(pattern).map_err(|e| format!("invalid glob pattern: {e}"))?);
        // Also accept patterns that omit the recursive prefix when the model
        // writes `*.rs` meaning "anywhere".
        if !pattern.contains('/') && !pattern.contains('\\') && !pattern.starts_with("**/") {
            let anywhere = format!("**/{pattern}");
            builder.add(Glob::new(&anywhere).map_err(|e| format!("invalid glob pattern: {e}"))?);
        }
        let set = builder
            .build()
            .map_err(|e| format!("invalid glob pattern: {e}"))?;

        let root = self.root.clone();
        let matches = tokio::task::spawn_blocking(move || collect_matches(&root, &set))
            .await
            .map_err(|e| format!("glob task failed: {e}"))??;

        format_matches(matches).map(super::ToolOutcome::text)
    }
}

fn collect_matches(root: &ProjectRoot, set: &globset::GlobSet) -> Result<Vec<String>, String> {
    let mut matches = Vec::new();

    // Stopping the walk, not just the collecting. The cap is what makes a glob
    // over a large repository cheap, and it only does that if the traversal
    // itself ends — the matching was never the expensive part.
    walk_files(root, root.as_path(), |resolved| {
        let rel = root.relativize(&resolved);
        if set.is_match(&rel) || set.is_match(Path::new(&rel)) {
            matches.push(rel);
            if matches.len() >= MAX_MATCHES {
                return false;
            }
        }
        true
    });

    // Sorted after the fact: the walk has no useful order, so the cap takes
    // whichever matches it reaches first and this makes the output stable to
    // read. Which 200 you get is unchanged from before.
    matches.sort();
    Ok(matches)
}

fn format_matches(matches: Vec<String>) -> Result<String, String> {
    let truncated = matches.len() >= MAX_MATCHES;
    if matches.is_empty() {
        return Ok("(no matches)".to_string());
    }
    let mut out = matches.join("\n");
    if truncated {
        out.push_str(&format!("\n\n[truncated at {MAX_MATCHES} matches]"));
    }
    Ok(out)
}

#[cfg(test)]
mod walk_bounds_tests {
    use super::*;
    use crate::tools::project::ProjectRoot;

    /// The cap has to end the traversal, not just the collecting. Counting how
    /// many files the walk actually visits is the only way to tell the two
    /// apart from the outside — the returned matches look identical either way.
    #[test]
    fn the_cap_stops_the_walk_rather_than_filtering_its_output() {
        let dir = std::env::temp_dir().join("zest-glob-cap");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Comfortably more than the cap, so a full walk is clearly visible.
        for index in 0..(MAX_MATCHES * 3) {
            std::fs::write(dir.join(format!("f{index:04}.txt")), "x").unwrap();
        }

        let root = ProjectRoot::new(&dir).unwrap();
        let mut visited = 0usize;
        crate::tools::walk::walk_files(&root, root.as_path(), |_| {
            visited += 1;
            visited < MAX_MATCHES
        });
        assert_eq!(visited, MAX_MATCHES, "the walk stopped when asked to");

        let set = globset::GlobSetBuilder::new()
            .add(globset::Glob::new("*.txt").unwrap())
            .build()
            .unwrap();
        let matches = collect_matches(&root, &set).unwrap();
        assert_eq!(matches.len(), MAX_MATCHES);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-glob-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        dir
    }

    #[tokio::test]
    async fn finds_rust_files() {
        let dir = scratch("rs");
        let tool = GlobFiles::new(&dir).unwrap();
        let out = tool
            .run(json!({ "pattern": "**/*.rs" }))
            .await
            .unwrap()
            .body;
        assert!(out.contains("src/main.rs"), "{out}");
        assert!(out.contains("src/lib.rs"), "{out}");
        assert!(!out.contains("README.md"), "{out}");
    }

    #[tokio::test]
    async fn bare_extension_pattern_matches_anywhere() {
        let dir = scratch("bare");
        let tool = GlobFiles::new(&dir).unwrap();
        let out = tool.run(json!({ "pattern": "*.md" })).await.unwrap().body;
        assert!(out.contains("README.md"), "{out}");
    }

    #[tokio::test]
    async fn rejects_empty_pattern() {
        let dir = scratch("empty");
        let tool = GlobFiles::new(&dir).unwrap();
        let err = tool.run(json!({ "pattern": "  " })).await.unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[tokio::test]
    async fn omits_sensitive_and_ignored() {
        let dir = scratch("omit");
        std::fs::write(dir.join(".env"), "x").unwrap();
        std::fs::write(dir.join(".env.example"), "x").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/out.rs"), "").unwrap();
        let tool = GlobFiles::new(&dir).unwrap();
        let out = tool.run(json!({ "pattern": "**/*" })).await.unwrap().body;
        assert!(!out.contains(".env\n") && !out.ends_with(".env"), "{out}");
        assert!(out.contains(".env.example"), "{out}");
        assert!(!out.contains("target/"), "{out}");
    }
}
