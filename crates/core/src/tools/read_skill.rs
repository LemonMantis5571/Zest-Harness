//! `read_skill` — load a discovered SKILL.md body by name.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::skills::SkillSet;

use super::Tool;

pub struct ReadSkill {
    skills: Arc<RwLock<SkillSet>>,
}

impl ReadSkill {
    pub fn new(skills: Arc<RwLock<SkillSet>>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl Tool for ReadSkill {
    fn name(&self) -> &str {
        "read_skill"
    }

    fn description(&self) -> &str {
        "Load the full instructions for a named personal skill \
(SKILL.md). Pass the skill `name` from the Available skills list."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name from the Available skills catalogue"
                }
            },
            "required": ["name"]
        })
    }

    async fn run(&self, input: Value) -> std::result::Result<super::ToolOutcome, String> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "read_skill requires a non-empty `name`".to_string())?;

        let skills = self
            .skills
            .read()
            .map_err(|_| "skill registry lock poisoned".to_string())?;

        match skills.get(name) {
            Some(skill) => Ok(super::ToolOutcome::text(format!(
                "# Skill: {}\n\n{}\n\n{}",
                skill.name,
                skill.description,
                skill.body.trim()
            ))),
            None => {
                let known: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
                if known.is_empty() {
                    Err(format!("unknown skill `{name}` (no skills loaded)"))
                } else {
                    Err(format!(
                        "unknown skill `{name}` (known: {})",
                        known.join(", ")
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{parse_skill_markdown, SkillSet};
    use std::path::Path;

    #[tokio::test]
    async fn read_skill_returns_body() {
        let mut set = SkillSet::default();
        set.insert(
            parse_skill_markdown(
                "---\nname: demo\ndescription: D\n---\n\nBody here.\n",
                Path::new("/demo/SKILL.md"),
            )
            .unwrap(),
        );
        let tool = ReadSkill::new(Arc::new(RwLock::new(set)));
        let out = tool.run(json!({ "name": "demo" })).await.unwrap().body;
        assert!(out.contains("Body here."));
    }

    #[tokio::test]
    async fn read_skill_unknown_errors() {
        let tool = ReadSkill::new(Arc::new(RwLock::new(SkillSet::default())));
        let err = tool.run(json!({ "name": "nope" })).await.unwrap_err();
        assert!(err.contains("unknown skill"), "{err}");
    }
}
