//! Session-scoped checklist (`todo_write`) so the model tracks multi-step work
//! and acceptance checks explicitly instead of relying on implicit working memory.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::outcome::ToolOutcome;
use super::Tool;

pub const TODO_WRITE_TOOL: &str = "todo_write";
const MAX_TODOS: usize = 32;
const MAX_CONTENT_CHARS: usize = 500;
const MAX_ID_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn is_unfinished(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }

    fn checkbox(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

pub type TodoStore = Arc<Mutex<Vec<TodoItem>>>;

pub fn format_todo_checklist(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "Checklist cleared (0 items).".to_string();
    }
    let completed = items
        .iter()
        .filter(|item| item.status == TodoStatus::Completed)
        .count();
    let mut out = format!("Checklist ({completed}/{} completed):\n", items.len());
    for item in items {
        out.push_str(&format!(
            "- {} {} ({})\n",
            item.status.checkbox(),
            item.content,
            item.id
        ));
    }
    out.trim_end().to_string()
}

pub fn unfinished_todo_items(store: &TodoStore) -> Vec<TodoItem> {
    store
        .lock()
        .map(|guard| {
            guard
                .iter()
                .filter(|item| item.status.is_unfinished())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

pub fn snapshot_todos(store: &TodoStore) -> Vec<TodoItem> {
    store.lock().map(|guard| guard.clone()).unwrap_or_default()
}

pub struct TodoWrite {
    store: TodoStore,
}

impl TodoWrite {
    pub fn new(store: TodoStore) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct TodoInput {
    todos: Vec<RawTodoItem>,
}

#[derive(Deserialize)]
struct RawTodoItem {
    #[serde(default)]
    id: Option<String>,
    content: String,
    status: TodoStatus,
}

fn parse_todos(input: Value) -> Result<Vec<TodoItem>, String> {
    let parsed: TodoInput =
        serde_json::from_value(input).map_err(|e| format!("invalid `todo_write` input: {e}"))?;
    if parsed.todos.len() > MAX_TODOS {
        return Err(format!(
            "at most {MAX_TODOS} checklist items are allowed (got {})",
            parsed.todos.len()
        ));
    }

    let mut seen = HashSet::with_capacity(parsed.todos.len());
    let mut out = Vec::with_capacity(parsed.todos.len());
    for (index, raw) in parsed.todos.into_iter().enumerate() {
        let content = raw.content.trim().to_string();
        if content.is_empty() {
            return Err(format!("item {} has empty `content`", index + 1));
        }
        if content.chars().count() > MAX_CONTENT_CHARS {
            return Err(format!(
                "item {} exceeds {MAX_CONTENT_CHARS} characters",
                index + 1
            ));
        }
        let id = raw
            .id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| (index + 1).to_string());
        if id.chars().count() > MAX_ID_CHARS {
            return Err(format!("item id `{id}` exceeds {MAX_ID_CHARS} characters"));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate checklist item id `{id}`"));
        }
        out.push(TodoItem {
            id,
            content,
            status: raw.status,
        });
    }
    Ok(out)
}

#[async_trait]
impl Tool for TodoWrite {
    fn name(&self) -> &str {
        TODO_WRITE_TOOL
    }

    fn description(&self) -> &str {
        "Create or update the active task checklist for multi-step work. Pass the complete \
         current list of items (`pending`, `in_progress`, or `completed`) so progress and \
         remaining verification steps stay explicit."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Complete ordered checklist for the current task (max 32 items).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Short stable identifier such as \"1\" or \"tests\"."
                            },
                            "content": {
                                "type": "string",
                                "description": "Concise description of the requirement or verification step."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            }
                        },
                        "required": ["content", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        })
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let items = parse_todos(input)?;
        let formatted = format_todo_checklist(&items);
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "todo store lock poisoned".to_string())?;
        *guard = items;
        Ok(ToolOutcome::text(formatted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn updates_and_formats_checklist_with_optional_ids() {
        let store: TodoStore = Arc::default();
        let tool = TodoWrite::new(store.clone());

        let out = tool
            .run(json!({
                "todos": [
                    { "id": "read", "content": "Inspect auth handler", "status": "completed" },
                    { "content": "Run cargo test", "status": "in_progress" }
                ]
            }))
            .await
            .unwrap();

        assert!(
            out.body.contains("Checklist (1/2 completed):"),
            "{}",
            out.body
        );
        assert!(out.body.contains("- [x] Inspect auth handler (read)"));
        assert!(out.body.contains("- [~] Run cargo test (2)"));
        assert_eq!(unfinished_todo_items(&store).len(), 1);
    }

    #[tokio::test]
    async fn rejects_duplicate_ids_and_empty_content() {
        let store: TodoStore = Arc::default();
        let tool = TodoWrite::new(store);

        let dup = tool
            .run(json!({
                "todos": [
                    { "id": "a", "content": "One", "status": "pending" },
                    { "id": "a", "content": "Two", "status": "pending" }
                ]
            }))
            .await
            .unwrap_err();
        assert!(dup.contains("duplicate"), "{dup}");

        let empty = tool
            .run(json!({
                "todos": [{ "id": "a", "content": "   ", "status": "pending" }]
            }))
            .await
            .unwrap_err();
        assert!(empty.contains("empty"), "{empty}");
    }
}
