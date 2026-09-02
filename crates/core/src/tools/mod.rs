pub mod approval;
pub mod bash;
pub mod browser;
pub mod capture;
pub mod delegate_feature;
pub mod edit_file;
pub mod external_agent;
pub mod glob_files;
pub mod grep;
pub(crate) mod isolated_workspace;
pub mod jobs;
pub mod list_dir;
pub mod outcome;
pub mod prepared;
pub mod project;
pub mod question;
pub mod read_file;
pub mod read_skill;
pub mod sensitive;
pub mod spill;
pub mod walk;
pub mod web_search;
pub mod write_file;

use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;

use crate::anthropic::types::{Message, ToolDef};
use crate::jobs::JobRegistry;
use crate::skills::SkillSet;

use self::approval::ToolRisk;
use self::bash::Bash;
use self::edit_file::EditFile;
use self::glob_files::GlobFiles;
use self::grep::Grep;
use self::list_dir::ListDir;
use self::prepared::PreparedToolCall;
use self::read_file::ReadFile;
use self::read_skill::ReadSkill;
use self::web_search::WebSearch;
use self::write_file::WriteFile;

pub use self::browser::{BrowserAction, BrowserAdapter, BrowserLocator, BrowserRequest};
pub use self::delegate_feature::{FeatureDelegator, DELEGATE_FEATURE_TOOL};
pub use self::outcome::{ToolMetadata, ToolOutcome};
pub use self::question::{
    parse_question_input, AskUser, DenyQuestioner, QuestionRequest, Questioner, ASK_USER_TOOL,
};

/// A client-side tool.
///
/// `run` / `execute_prepared` return `Result<ToolOutcome, String>` rather than a
/// harness error type on purpose: a tool failing is a normal conversational
/// event, not a harness failure. The `Err` string goes back to the model as a
/// `tool_result` with `is_error: true` so it can adapt, rather than aborting
/// the turn. Optional [`ToolMetadata`] rides beside the body for UI/persistence
/// and is never injected into the Messages API wire as structured content.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;

    /// Defaults to read — safe tools do not need an approval prompt.
    fn risk(&self) -> ToolRisk {
        ToolRisk::Read
    }

    /// Refresh model-visible conversation context before a tool batch runs.
    /// Most tools are stateless; orchestration tools can project this into a
    /// bounded handoff for a worker.
    fn update_context(&self, _messages: &[Message]) {}

    fn uses_context(&self) -> bool {
        false
    }

    /// Build a prepared call once before optional approval + execution.
    ///
    /// Write tools snapshot path, pre-image, and diff here so approval and
    /// execution share one coherent plan.
    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        Ok(PreparedToolCall::plain(self.name(), self.risk(), input))
    }

    /// Execute a previously prepared call (after approval when required).
    async fn execute_prepared(
        &self,
        prepared: PreparedToolCall,
    ) -> std::result::Result<ToolOutcome, String> {
        match prepared.plain_input() {
            Some(input) => self.run(input.clone()).await,
            None => Err(format!(
                "tool `{}` cannot execute this prepared call",
                prepared.tool_name
            )),
        }
    }

    async fn run(&self, input: Value) -> std::result::Result<ToolOutcome, String>;
}

/// Cloning shares the tools themselves — cheap, and how a delegated worker gets
/// its own registry without rebuilding anything.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    /// Attached by [`crate::runtime::RuntimeBuilder`]. `None` — what every test
    /// and every headless caller gets — keeps every result inline.
    ///
    /// A field here rather than a decorating [`Tool`] because dispatch is the
    /// one place both call paths meet: the concurrent batch awaits
    /// [`Self::execute_prepared`] directly and never passes through the agent's
    /// gated wrapper. A decorator would also have to forward every trait method,
    /// and forgetting one fails silently — a missed `uses_context` quietly stops
    /// delegation from seeing conversation context, and a missed `input_schema`
    /// corrupts the tool list at the front of the cached prompt prefix.
    spill: Option<Arc<spill::SpillPolicy>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Keep oversized results out of context by storing them under the given
    /// policy.
    pub fn with_spill(mut self, policy: Arc<spill::SpillPolicy>) -> Self {
        self.spill = Some(policy);
        self
    }

    /// Project-relative directory oversized results are stored in, when a
    /// front-end supplied a conversation to store them under.
    pub fn spill_dir(&self) -> Option<&str> {
        self.spill.as_ref().map(|policy| policy.spill_dir())
    }

    /// Stable order — the tool list renders at the very front of the prompt, so
    /// reordering it invalidates the entire prompt cache.
    pub fn definitions(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                // Set by the provider, which is the only layer that knows
                // whether the endpoint understands caching.
                cache_control: None,
            })
            .collect()
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    pub fn risk(&self, name: &str) -> Option<ToolRisk> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.risk())
    }

    pub fn update_context(&self, messages: &[Message]) {
        for tool in &self.tools {
            if tool.uses_context() {
                tool.update_context(messages);
            }
        }
    }

    pub fn uses_context(&self) -> bool {
        self.tools.iter().any(|tool| tool.uses_context())
    }

    /// Whether any of the tools about to run actually consumes conversation
    /// context.
    ///
    /// Distinct from [`Self::uses_context`], which answers "is such a tool
    /// registered at all". Preparing the context means cloning and redacting the
    /// whole conversation, so doing it because delegation is *configured* — as
    /// opposed to because it is *about to happen* — pays that cost on every tool
    /// round of every turn, in sessions that may never delegate once.
    pub fn round_uses_context(&self, called: &[&str]) -> bool {
        self.tools
            .iter()
            .any(|tool| tool.uses_context() && called.contains(&tool.name()))
    }

    pub fn prepare(&self, name: &str, input: Value) -> Result<PreparedToolCall, String> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.prepare(input),
            None => Err(format!("unknown tool: {name}")),
        }
    }

    pub async fn execute_prepared(
        &self,
        prepared: PreparedToolCall,
    ) -> std::result::Result<ToolOutcome, String> {
        let name = prepared.tool_name.clone();
        let risk = prepared.risk;
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => match tool.execute_prepared(prepared).await {
                // An error is not a result: it carries corrective feedback the
                // model needs verbatim, and it is short by construction.
                Err(message) => Err(message),
                Ok(outcome) => Ok(match &self.spill {
                    Some(policy) => policy.apply(&name, risk, outcome),
                    None => outcome,
                }),
            },
            None => Err(format!("unknown tool: {name}")),
        }
    }

    pub async fn run(&self, name: &str, input: Value) -> std::result::Result<ToolOutcome, String> {
        let prepared = self.prepare(name, input)?;
        self.execute_prepared(prepared).await
    }
}

/// Register the project-scoped read-only tools (`read_file`, `list_dir`, `glob`,
/// `grep`) plus network `web_search`. Order is stable so prompt-cache prefixes
/// stay warm.
pub fn register_read_tools(
    registry: &mut ToolRegistry,
    root: impl AsRef<Path>,
) -> std::io::Result<()> {
    let root = root.as_ref();
    registry.register(Arc::new(ReadFile::new(root)?));
    registry.register(Arc::new(ListDir::new(root)?));
    registry.register(Arc::new(GlobFiles::new(root)?));
    registry.register(Arc::new(Grep::new(root)?));
    registry.register(Arc::new(WebSearch::new()));
    Ok(())
}

/// Register the parent-only local browser tool after the shared worker tools
/// have been cloned, so delegated workers cannot control the desktop webview.
pub fn register_browser_tool(registry: &mut ToolRegistry, adapter: Arc<dyn BrowserAdapter>) {
    browser::register_browser_tool(registry, adapter);
}

/// Register `read_skill` against a shared skill registry (hot-reloadable).
pub fn register_skill_tools(registry: &mut ToolRegistry, skills: Arc<RwLock<SkillSet>>) {
    registry.register(Arc::new(ReadSkill::new(skills)));
}

/// Register the provider-independent tool that pauses for a user's answer.
/// Workers deliberately do not receive it; only the parent desktop turn owns a
/// human interaction surface.
pub fn register_question_tool(registry: &mut ToolRegistry) {
    question::register_question_tool(registry);
}

/// Register project-scoped write tools (`write_file`, `edit_file`). Requires an
/// [`Approver`] on the agent — without one, gated calls are denied.
///
/// `edit_file` goes last so adding it shifts the cached prompt prefix exactly
/// once rather than displacing every tool after it.
pub fn register_write_tools(
    registry: &mut ToolRegistry,
    root: impl AsRef<Path>,
) -> std::io::Result<()> {
    let root = root.as_ref();
    registry.register(Arc::new(WriteFile::new(root)?));
    registry.register(Arc::new(EditFile::new(root)?));
    Ok(())
}

/// Register `bash`, scoped to `root`.
///
/// Separate from the write tools because it is separately configurable and
/// separately refusable: a front-end with no [`Approver`] should not offer it
/// at all, since every non-allowlisted command would be auto-denied.
pub fn register_exec_tools(
    registry: &mut ToolRegistry,
    root: impl AsRef<Path>,
    settings: self::bash::BashSettings,
) -> std::io::Result<()> {
    register_exec_tools_with_jobs(registry, root, settings, Arc::new(JobRegistry::new()), None)
}

/// Register shell execution plus the shared background-job controls. The
/// registry is supplied by the front-end so a new runtime does not orphan
/// jobs started by the previous one.
pub fn register_exec_tools_with_jobs(
    registry: &mut ToolRegistry,
    root: impl AsRef<Path>,
    settings: self::bash::BashSettings,
    jobs: Arc<JobRegistry>,
    owner_thread_id: Option<String>,
) -> std::io::Result<()> {
    let mut bash = Bash::new(root)?
        .with_settings(settings)
        .with_job_registry(jobs.clone());
    if let Some(owner) = owner_thread_id.clone() {
        bash = bash.with_job_owner(owner);
    }
    registry.register(Arc::new(bash));
    jobs::register_job_tools(registry, jobs, owner_thread_id);
    Ok(())
}

/// Register only the model-facing job controls, useful when a provider owns
/// its own shell but Zest still needs to expose jobs created by another tool.
pub fn register_job_tools(
    registry: &mut ToolRegistry,
    jobs: Arc<JobRegistry>,
    owner_thread_id: Option<String>,
) {
    jobs::register_job_tools(registry, jobs, owner_thread_id);
}

#[cfg(test)]
mod characterization {
    use super::*;

    fn scratch(name: &str) -> crate::fsutil::ScratchDir {
        crate::fsutil::ScratchDir::new(&format!("zest-tools-char-{name}-"))
    }

    #[test]
    fn read_tools_default_to_read_risk() {
        let dir = scratch("read-risk");
        let mut reg = ToolRegistry::new();
        register_read_tools(&mut reg, &dir).unwrap();
        for name in ["read_file", "list_dir", "glob", "grep", "web_search"] {
            assert_eq!(reg.risk(name), Some(ToolRisk::Read), "{name}");
            assert!(!reg.risk(name).unwrap().requires_approval(), "{name}");
        }
    }

    #[test]
    fn write_tools_register_in_cache_stable_order() {
        let dir = scratch("write-order");
        let mut reg = ToolRegistry::new();
        register_read_tools(&mut reg, &dir).unwrap();
        register_write_tools(&mut reg, &dir).unwrap();
        assert_eq!(
            reg.names(),
            vec![
                "read_file",
                "list_dir",
                "glob",
                "grep",
                "web_search",
                "write_file",
                "edit_file",
            ]
        );
    }

    #[test]
    fn edit_tool_prepare_reuses_the_write_path() {
        let dir = scratch("edit-prep");
        std::fs::write(dir.join("f.txt"), "before\n").unwrap();
        let mut reg = ToolRegistry::new();
        register_write_tools(&mut reg, &dir).unwrap();
        assert_eq!(reg.risk("edit_file"), Some(ToolRisk::Write));
        let prepared = reg
            .prepare(
                "edit_file",
                serde_json::json!({
                    "path": "f.txt",
                    "old_string": "before",
                    "new_string": "after"
                }),
            )
            .unwrap();
        // Dispatch must come back to edit_file, not to write_file, even though
        // the prepared kind is shared.
        assert_eq!(prepared.tool_name, "edit_file");
        assert_eq!(prepared.risk, ToolRisk::Write);
        assert!(prepared.preview.diff.contains("+after"));
    }

    #[test]
    fn write_tool_prepare_carries_write_risk_and_preview() {
        let dir = scratch("write-prep");
        let mut reg = ToolRegistry::new();
        register_write_tools(&mut reg, &dir).unwrap();
        assert_eq!(reg.risk("write_file"), Some(ToolRisk::Write));
        let prepared = reg
            .prepare(
                "write_file",
                serde_json::json!({ "path": "f.txt", "content": "x" }),
            )
            .unwrap();
        assert_eq!(prepared.risk, ToolRisk::Write);
        assert!(prepared.risk.requires_approval());
        assert_eq!(prepared.preview.path, "f.txt");
        assert!(!prepared.preview.summary.is_empty());
    }

    #[test]
    fn unknown_tool_risk_is_none_and_prepare_errors() {
        let reg = ToolRegistry::new();
        assert_eq!(reg.risk("missing"), None);
        let err = reg.prepare("missing", serde_json::json!({})).unwrap_err();
        assert!(err.contains("unknown tool"), "{err}");
    }

    #[test]
    fn shared_job_tools_are_registered_with_owner_safe_risk() {
        let mut reg = ToolRegistry::new();
        register_job_tools(
            &mut reg,
            Arc::new(JobRegistry::new()),
            Some("thread-a".into()),
        );
        assert_eq!(reg.names(), vec!["job_list", "job_output", "job_kill"]);
        assert_eq!(reg.risk("job_list"), Some(ToolRisk::Read));
        assert_eq!(reg.risk("job_output"), Some(ToolRisk::Read));
        assert_eq!(reg.risk("job_kill"), Some(ToolRisk::Exec));
    }

    /// A tool that returns a body of a requested size, or an error.
    struct Sized {
        name: &'static str,
        risk: ToolRisk,
    }

    #[async_trait]
    impl Tool for Sized {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "test"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        fn risk(&self) -> ToolRisk {
            self.risk
        }
        async fn run(&self, input: Value) -> std::result::Result<ToolOutcome, String> {
            let bytes = input.get("bytes").and_then(Value::as_u64).unwrap_or(0) as usize;
            if input.get("fail").is_some() {
                return Err("x".repeat(bytes));
            }
            Ok(ToolOutcome::text("x".repeat(bytes)))
        }
    }

    fn spilling_registry(dir: &std::path::Path, cap: usize, risk: ToolRisk) -> ToolRegistry {
        let store = self::spill::SpillStore::open(dir, "t-1").unwrap();
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Sized { name: "big", risk }));
        reg.register(Arc::new(Sized {
            name: read_file::READ_FILE_TOOL,
            risk: ToolRisk::Read,
        }));
        reg.with_spill(Arc::new(self::spill::SpillPolicy::new(store, cap)))
    }

    #[tokio::test]
    async fn spilling_is_off_until_a_store_is_attached() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Sized {
            name: "big",
            risk: ToolRisk::Read,
        }));
        assert_eq!(reg.spill_dir(), None);
        let out = reg
            .run("big", serde_json::json!({ "bytes": 200_000 }))
            .await
            .unwrap();
        assert_eq!(
            out.body.len(),
            200_000,
            "an unattached registry must not bound"
        );
    }

    /// The concurrent batch awaits `execute_prepared` directly rather than going
    /// through the agent's gated wrapper, so the hook has to live at dispatch.
    /// This drives the same entry point both paths use.
    #[tokio::test]
    async fn dispatch_bounds_an_oversized_result() {
        let dir = scratch("spill-dispatch");
        let reg = spilling_registry(&dir, 2_048, ToolRisk::Read);
        assert_eq!(reg.spill_dir(), Some(".zest/spill/t-1"));
        let out = reg
            .run("big", serde_json::json!({ "bytes": 200_000 }))
            .await
            .unwrap();
        assert!(out.body.len() <= 2_048, "{}", out.body.len());
        assert!(out.body.contains("Full result stored at"), "{}", out.body);
    }

    #[tokio::test]
    async fn dispatch_leaves_a_result_within_the_cap_alone() {
        let dir = scratch("spill-small");
        let reg = spilling_registry(&dir, 2_048, ToolRisk::Read);
        let out = reg
            .run("big", serde_json::json!({ "bytes": 100 }))
            .await
            .unwrap();
        assert_eq!(out.body.len(), 100);
        assert!(!dir.join(".zest").exists());
    }

    #[tokio::test]
    async fn a_tool_error_is_never_spilled() {
        let dir = scratch("spill-err");
        let reg = spilling_registry(&dir, 2_048, ToolRisk::Read);
        let err = reg
            .run("big", serde_json::json!({ "bytes": 200_000, "fail": true }))
            .await
            .unwrap_err();
        assert_eq!(
            err.len(),
            200_000,
            "corrective feedback must reach the model verbatim"
        );
        assert!(!dir.join(".zest").exists());
    }

    #[tokio::test]
    async fn dispatch_never_spills_the_read_tool() {
        let dir = scratch("spill-read");
        let reg = spilling_registry(&dir, 2_048, ToolRisk::Read);
        let out = reg
            .run(
                read_file::READ_FILE_TOOL,
                serde_json::json!({ "bytes": 200_000 }),
            )
            .await
            .unwrap();
        assert_eq!(out.body.len(), 200_000);
        assert!(!dir.join(".zest").exists());
    }

    /// The whole promise, end to end, through the real tools: a real `grep` over
    /// a tree big enough to exceed the cap, then the real `read_file` retrieving
    /// the stored output back through the locator the model was handed.
    ///
    /// Everything else here uses a fake tool and asserts the policy in isolation.
    /// This is the one test that proves the loop actually closes — that the
    /// locator is a path the read tools accept, resolves inside the project root,
    /// and needs no approval.
    #[tokio::test]
    async fn a_real_grep_spill_can_be_read_back_through_its_locator() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // `grep` stops at 100 matches and clips each line to 400 chars, so its
        // output only reaches the 32 KiB cap when the matching lines are long.
        // Long lines are exactly the case worth keeping: a minified bundle or a
        // one-line JSON blob.
        for file in 0..120 {
            let body = format!("needle {file} {}\n", "wide-".repeat(90));
            std::fs::write(dir.join(format!("f{file}.txt")), body).unwrap();
        }

        let store = self::spill::SpillStore::open(dir, "t-1").unwrap();
        let mut reg = ToolRegistry::new();
        register_read_tools(&mut reg, dir).unwrap();
        let reg = reg.with_spill(Arc::new(self::spill::SpillPolicy::new(store, 32 * 1024)));

        let out = reg
            .run("grep", serde_json::json!({ "pattern": "needle" }))
            .await
            .unwrap();
        assert_ne!(
            out.body.as_str(),
            "(no matches)",
            "grep walked none of the fixture files in {}",
            dir.display()
        );
        assert!(out.body.len() <= 32 * 1024, "{}", out.body.len());

        let locator = out
            .body
            .split("Full result stored at: ")
            .nth(1)
            .and_then(|rest| rest.split(". Use").next())
            .expect("the model must be handed a locator");

        // Read it back exactly as the notice tells the model to.
        let back = reg
            .run(
                read_file::READ_FILE_TOOL,
                serde_json::json!({ "path": locator }),
            )
            .await
            .expect("the locator must be readable, with no approval needed");
        assert!(back.body.contains("needle"), "retrieved nothing useful");

        // And grep it, the other half of the hint.
        let searched = reg
            .run(
                "grep",
                serde_json::json!({ "pattern": "needle 41", "path": locator }),
            )
            .await
            .expect("the locator must be greppable");
        assert!(searched.body.contains("needle 41"), "{}", searched.body);
    }

    #[tokio::test]
    async fn dispatch_never_spills_a_sensitive_result() {
        let dir = scratch("spill-sensitive");
        let reg = spilling_registry(&dir, 2_048, ToolRisk::Sensitive);
        let prepared = reg
            .prepare("big", serde_json::json!({ "bytes": 200_000 }))
            .unwrap();
        assert_eq!(prepared.risk, ToolRisk::Sensitive);
        let out = reg.execute_prepared(prepared).await.unwrap();
        assert_eq!(out.body.len(), 200_000);
        assert!(
            !dir.join(".zest").exists(),
            "a second cleartext copy must never be written"
        );
    }
}
