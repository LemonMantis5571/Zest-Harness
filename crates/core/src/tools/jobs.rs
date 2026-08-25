//! Model-facing views over the shared [`JobRegistry`](crate::JobRegistry).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::jobs::JobRegistry;

use super::approval::ToolRisk;
use super::{Tool, ToolOutcome};

const MAX_WAIT_MS: u64 = 30_000;

pub struct JobList {
    registry: Arc<JobRegistry>,
    owner_thread_id: Option<String>,
}

pub struct JobOutput {
    registry: Arc<JobRegistry>,
    owner_thread_id: Option<String>,
}

pub struct JobKill {
    registry: Arc<JobRegistry>,
    owner_thread_id: Option<String>,
}

pub fn register_job_tools(
    registry: &mut super::ToolRegistry,
    jobs: Arc<JobRegistry>,
    owner_thread_id: Option<String>,
) {
    registry.register(Arc::new(JobList {
        registry: jobs.clone(),
        owner_thread_id: owner_thread_id.clone(),
    }));
    registry.register(Arc::new(JobOutput {
        registry: jobs.clone(),
        owner_thread_id: owner_thread_id.clone(),
    }));
    registry.register(Arc::new(JobKill {
        registry: jobs,
        owner_thread_id,
    }));
}

#[async_trait]
impl Tool for JobList {
    fn name(&self) -> &str {
        "job_list"
    }

    fn description(&self) -> &str {
        "List background jobs owned by this chat, including running, completed, failed, and killed jobs."
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }

    async fn run(&self, _input: Value) -> Result<ToolOutcome, String> {
        let jobs = self.registry.list(self.owner_thread_id.as_deref());
        serde_json::to_string_pretty(&jobs)
            .map(ToolOutcome::text)
            .map_err(|error| format!("could not serialize job list: {error}"))
    }
}

#[async_trait]
impl Tool for JobOutput {
    fn name(&self) -> &str {
        "job_output"
    }

    fn description(&self) -> &str {
        "Read incremental output from a background job. Pass the previous next_offset to avoid rereading old output; set wait to true to wait for new output or completion."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "job_id": {"type":"string"},
                "offset": {"type":"integer","minimum":0},
                "wait": {"type":"boolean"},
                "timeout_ms": {"type":"integer","minimum":1,"maximum":MAX_WAIT_MS}
            },
            "required":["job_id"],
            "additionalProperties":false
        })
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let job_id = input
            .get("job_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "missing required field `job_id`".to_string())?;
        let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let wait = input.get("wait").and_then(Value::as_bool).unwrap_or(false);
        let timeout = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(MAX_WAIT_MS)
            .min(MAX_WAIT_MS);
        let read = if wait {
            self.registry
                .read_wait(
                    job_id,
                    self.owner_thread_id.as_deref(),
                    offset,
                    Some(Duration::from_millis(timeout)),
                )
                .await?
        } else {
            self.registry
                .read(job_id, self.owner_thread_id.as_deref(), offset)
                .await?
        };
        serde_json::to_string_pretty(&read)
            .map(ToolOutcome::text)
            .map_err(|error| format!("could not serialize job output: {error}"))
    }
}

#[async_trait]
impl Tool for JobKill {
    fn name(&self) -> &str {
        "job_kill"
    }

    fn description(&self) -> &str {
        "Stop a running background job owned by this chat."
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Exec
    }

    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "job_id": {"type":"string"},
                "reason": {"type":"string"}
            },
            "required":["job_id"],
            "additionalProperties":false
        })
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let job_id = input
            .get("job_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "missing required field `job_id`".to_string())?;
        let reason = input.get("reason").and_then(Value::as_str);
        let snapshot = self
            .registry
            .kill(job_id, self.owner_thread_id.as_deref(), reason)
            .await?;
        serde_json::to_string_pretty(&snapshot)
            .map(ToolOutcome::text)
            .map_err(|error| format!("could not serialize job state: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobRead;
    use crate::tools::ToolRegistry;
    use serde_json::json;
    use std::path::Path;

    fn output_command() -> &'static str {
        if cfg!(windows) {
            "echo job-output"
        } else {
            "printf job-output"
        }
    }

    #[tokio::test]
    async fn model_job_tools_list_read_and_fence_by_owner() {
        let jobs = Arc::new(JobRegistry::new());
        let job = jobs
            .start_process(
                output_command(),
                Path::new("."),
                "test",
                "tool test",
                Some("thread-a".into()),
            )
            .await
            .unwrap();

        let mut owned = ToolRegistry::new();
        register_job_tools(&mut owned, jobs.clone(), Some("thread-a".into()));
        let listed = owned.run("job_list", json!({})).await.unwrap();
        assert!(listed.body.contains(&job.id));

        let output = owned
            .run(
                "job_output",
                json!({ "job_id": job.id, "wait": true, "timeout_ms": 5_000 }),
            )
            .await
            .unwrap();
        let read: JobRead = serde_json::from_str(&output.body).unwrap();
        assert!(read.text.contains("job-output"), "{}", read.text);
        assert_eq!(read.snapshot.owner_thread_id.as_deref(), Some("thread-a"));

        let mut other = ToolRegistry::new();
        register_job_tools(&mut other, jobs, Some("thread-b".into()));
        assert!(other
            .run("job_output", json!({ "job_id": job.id }))
            .await
            .is_err());
    }
}
