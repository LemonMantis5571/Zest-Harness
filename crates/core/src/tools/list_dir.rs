use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::project::ProjectRoot;
use super::walk::list_children;
use super::Tool;

const MAX_ENTRIES: usize = 500;

/// List a directory, confined to a project root.
pub struct ListDir {
    root: ProjectRoot,
}

impl ListDir {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: ProjectRoot::new(root)?,
        })
    }
}

#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List entries in a directory under the project root. Returns names with \
         a trailing `/` for directories. Respects `.gitignore` and omits \
         sensitive files (e.g. `.env`) and hard-skipped dirs (`.git`, `.zest`, \
         `target`, `node_modules`). Paths are relative to the project root."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory relative to the project root. Defaults to `.`."
                }
            },
            "additionalProperties": false
        })
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let path = input.get("path").and_then(Value::as_str).unwrap_or(".");

        let resolved = self.root.resolve(path)?;
        let meta = tokio::fs::metadata(&resolved)
            .await
            .map_err(|e| format!("stat failed: {e}"))?;
        if !meta.is_dir() {
            return Err(format!("`{path}` is not a directory"));
        }

        let root = self.root.clone();
        let entries = tokio::task::spawn_blocking(move || list_children(&root, &resolved))
            .await
            .map_err(|e| format!("list_dir task failed: {e}"))??;

        let truncated = entries.len() >= MAX_ENTRIES;
        let mut lines: Vec<String> = entries
            .into_iter()
            .take(MAX_ENTRIES)
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.name)
                } else {
                    e.name
                }
            })
            .collect();
        lines.sort();
        let mut out = lines.join("\n");
        if out.is_empty() {
            out = "(empty)".to_string();
        }
        if truncated {
            out.push_str(&format!("\n\n[truncated at {MAX_ENTRIES} entries]"));
        }
        Ok(super::ToolOutcome::text(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-list-dir-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn lists_files_and_directories() {
        let dir = scratch("basic");
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let tool = ListDir::new(&dir).unwrap();
        let out = tool.run(json!({})).await.unwrap().body;
        assert_eq!(out, "a.txt\nb.txt\nsub/");
    }

    #[tokio::test]
    async fn omits_sensitive_files() {
        let dir = scratch("secret");
        std::fs::write(dir.join(".env"), "x").unwrap();
        std::fs::write(dir.join("visible.txt"), "x").unwrap();
        let tool = ListDir::new(&dir).unwrap();
        let out = tool.run(json!({})).await.unwrap().body;
        assert!(out.contains("visible.txt"), "{out}");
        assert!(!out.contains(".env"), "{out}");
    }

    #[tokio::test]
    async fn rejects_files_and_escapes() {
        let dir = scratch("reject");
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        let tool = Arc::new(ListDir::new(&dir).unwrap());

        let err = tool.run(json!({ "path": "f.txt" })).await.unwrap_err();
        assert!(err.contains("not a directory"), "{err}");

        let err = tool.run(json!({ "path": ".." })).await.unwrap_err();
        assert!(
            err.contains("outside the project root") || err.contains("cannot resolve"),
            "{err}"
        );
    }
}
