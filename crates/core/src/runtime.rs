//! Shared assembly for CLI and desktop front-ends.
//!
//! One place for config → registry → tools → agent so both entrypoints stay
//! aligned. Delegated work uses explicitly configured external ACP/CLI
//! workers; the parent conversation stays pinned to a single provider.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use crate::agent::Agent;
use crate::config::Config;
use crate::error::{HarnessError, Result};
use crate::mcp::register_mcp_tools;
use crate::prompt::{
    compose_system_with_docs, env_context, load_custom_system, load_project_docs, DEFAULT_SYSTEM,
    LOCAL_BROWSER_SYSTEM,
};
use crate::provider::normalize_effort;
use crate::provider::registry::{ProviderRegistry, Skipped};
use crate::provider::SystemPrompt;
use crate::skills::SkillSet;
use crate::tools::approval::{AllowApprover, ApprovalMode, ApprovalPolicy, Approver, DenyApprover};
use crate::tools::external_agent::ExternalAgent;
use crate::tools::question::{DenyQuestioner, Questioner};
use crate::tools::spill::{SpillPolicy, SpillStore};
use crate::tools::{
    register_browser_tool, register_exec_tools, register_question_tool, register_read_tools,
    register_skill_tools, register_write_tools, BrowserAdapter, FeatureDelegator, ToolRegistry,
};
use crate::usage::Ledger;

/// Built runtime ready for a provider-pinned conversation.
pub struct RuntimeSession {
    pub root: PathBuf,
    pub config: Config,
    pub registry: Arc<ProviderRegistry>,
    pub provider_id: String,
    pub model: String,
    pub effort: String,
    pub agent: Agent,
    pub ledger: Arc<Mutex<Ledger>>,
    /// Shared with the agent; flip the mode here to change it mid-session.
    pub policy: Arc<Mutex<ApprovalPolicy>>,
    /// Shared with `read_skill`; can be replaced on Settings save.
    pub skills: Arc<RwLock<SkillSet>>,
    /// Base system prompt before custom/skills layers (front-end flavor).
    pub base_system: String,
    /// Non-fatal things the front-end should surface — chiefly a remembered
    /// model or effort that had to be dropped. Silently correcting a stored
    /// preference would leave the user wondering why the picker moved.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeRole {
    #[default]
    Parent,
    DelegationWorker,
    DelegationReviewer,
}

/// A configured provider target after credentials and capability validation.
/// The value deliberately contains no credential material.
pub struct ResolvedProviderTarget {
    pub provider_id: String,
    pub model: String,
    pub effort: String,
    pub provider: Arc<dyn crate::provider::Provider>,
}

/// Assembles config, providers, tools, ledger, and an [`Agent`].
pub struct RuntimeBuilder {
    root: PathBuf,
    config: Option<Config>,
    provider_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    /// Sticky model/effort from a previous session. Unlike [`Self::model`] this
    /// is dropped rather than fatal when it does not fit the provider.
    remembered: Option<(Option<String>, Option<String>)>,
    system: Option<String>,
    ledger: Option<Arc<Mutex<Ledger>>>,
    approver: Option<Arc<dyn Approver>>,
    questioner: Option<Arc<dyn Questioner>>,
    policy: Option<Arc<Mutex<ApprovalPolicy>>>,
    browser: Option<Arc<dyn BrowserAdapter>>,
    enable_external_agents: bool,
    parent_thread_id: Option<String>,
    register_write: bool,
    register_exec: bool,
    role: RuntimeRole,
}

impl RuntimeBuilder {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            config: None,
            provider_id: None,
            model: None,
            effort: None,
            remembered: None,
            system: None,
            ledger: None,
            approver: None,
            questioner: None,
            policy: None,
            browser: None,
            enable_external_agents: true,
            parent_thread_id: None,
            register_write: true,
            register_exec: true,
            role: RuntimeRole::Parent,
        }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_provider(mut self, id: impl Into<String>) -> Self {
        self.provider_id = Some(id.into());
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// Sticky model/effort restored from a previous session.
    ///
    /// Separate from [`Self::with_model`] because the two deserve opposite
    /// treatment when they do not fit the provider. A model the user just
    /// picked should fail loudly. A *remembered* one must not: a stale value on
    /// disk would otherwise make the provider permanently unselectable, and the
    /// only way to change it is to start a session you can no longer start.
    pub fn with_remembered_options(
        mut self,
        model: Option<String>,
        effort: Option<String>,
    ) -> Self {
        self.remembered = Some((model, effort));
        self
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_ledger(mut self, ledger: Arc<Mutex<Ledger>>) -> Self {
        self.ledger = Some(ledger);
        self
    }

    pub fn with_approver(mut self, approver: Arc<dyn Approver>) -> Self {
        self.approver = Some(approver);
        self
    }

    /// Share the front-end hook used by the provider-independent `ask_user`
    /// tool. Omitted means headless callers do not advertise that tool.
    pub fn with_questioner(mut self, questioner: Arc<dyn Questioner>) -> Self {
        self.questioner = Some(questioner);
        self
    }

    /// Share the permission policy so the front-end can switch mode later.
    /// Omitted means [`ApprovalMode::Manual`](crate::ApprovalMode::Manual).
    pub fn with_policy(mut self, policy: Arc<Mutex<ApprovalPolicy>>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Attach a local browser session to the parent agent. The adapter is
    /// deliberately optional so the CLI and headless callers do not acquire a
    /// desktop dependency or advertise a tool they cannot execute.
    pub fn with_browser_adapter(mut self, browser: Arc<dyn BrowserAdapter>) -> Self {
        self.browser = Some(browser);
        self
    }

    pub fn enable_external_agents(mut self, on: bool) -> Self {
        self.enable_external_agents = on;
        self
    }

    /// Stable coordinator identity used by feature-card jobs. Direct CLI
    /// callers may omit it; the desktop supplies the durable thread id.
    pub fn with_parent_thread_id(mut self, thread_id: impl Into<String>) -> Self {
        self.parent_thread_id = Some(thread_id.into());
        self
    }

    pub fn register_write_tools(mut self, on: bool) -> Self {
        self.register_write = on;
        self
    }

    /// Off for callers that must not run commands regardless of config —
    /// `doctor --live` and delegated workers.
    pub fn register_exec_tools(mut self, on: bool) -> Self {
        self.register_exec = on;
        self
    }

    pub fn with_role(mut self, role: RuntimeRole) -> Self {
        self.role = role;
        self
    }

    pub fn build(self) -> Result<RuntimeSession> {
        let root = self.root;
        let config = match self.config {
            Some(c) => c,
            None => Config::find(&root)?,
        };

        let (registry, skipped) = ProviderRegistry::from_config_at(&config, &root);

        let provider_id = self
            .provider_id
            .or_else(|| config.default_target().map(|t| t.provider.clone()))
            .ok_or_else(|| {
                HarnessError::Other(
                    "no provider selected and zest.toml has no [default].provider".into(),
                )
            })?;

        let provider = match registry.get(&provider_id) {
            Some(provider) => provider,
            // Two very different failures used to share one message. Telling
            // them apart is the difference between "set a key" and "this folder
            // has no config", and the registry already worked out which.
            None => {
                return Err(HarnessError::Other(unloadable_provider(
                    &provider_id,
                    &config,
                    &skipped,
                    &root,
                )))
            }
        };

        if self.role != RuntimeRole::Parent && provider.owns_agent_loop() {
            let role = match self.role {
                RuntimeRole::DelegationWorker => "worker",
                RuntimeRole::DelegationReviewer => "reviewer",
                RuntimeRole::Parent => "parent",
            };
            return Err(HarnessError::Other(format!(
                "provider `{provider_id}` owns its agent loop and cannot run as a native delegated {role}"
            )));
        }

        let (remembered_model, remembered_effort) = self.remembered.unwrap_or((None, None));
        let mut warnings: Vec<String> = Vec::new();

        // A remembered model that this provider cannot serve is discarded here,
        // before it can reach validation and make the provider unreachable.
        // Cross-provider bleed is the usual cause: a Codex model left in a
        // Claude slot by an old single-provider preference file.
        let remembered_model = remembered_model.filter(|m| {
            let ok = provider.validate_selection(m, "high").is_ok()
                || provider.models().iter().any(|spec| spec.id == *m);
            if !ok {
                warnings.push(format!(
                    "ignored remembered model `{m}`: provider `{provider_id}` does not offer it"
                ));
            }
            ok
        });

        let model = self
            .model
            .or(remembered_model)
            .or_else(|| {
                config.default_target().and_then(|t| {
                    if t.provider == provider_id {
                        t.model.clone()
                    } else {
                        None
                    }
                })
            })
            // ZEST_MODEL is a global override and cannot know which provider it
            // is being applied to, so it only counts when it actually fits.
            .or_else(|| {
                std::env::var("ZEST_MODEL")
                    .ok()
                    .filter(|m| provider.models().iter().any(|spec| spec.id == *m))
            })
            .unwrap_or_else(|| provider.default_model().to_string());

        // An effort the caller passed in must fail loudly, exactly like a model
        // they passed in. Only inherited values get the soft landing.
        let effort_is_explicit = self.effort.is_some();
        let effort_source = self
            .effort
            .or(remembered_effort)
            .or_else(|| std::env::var("ZEST_EFFORT").ok())
            .unwrap_or_else(|| "high".to_string());
        let mut effort = normalize_effort(&effort_source);

        // Same reasoning as the model: an inherited value must not strand the
        // provider, because the only way to change it is a session you can no
        // longer start.
        if !effort_is_explicit && provider.validate_selection(&model, &effort).is_err() {
            let fallback = provider
                .models()
                .iter()
                .find(|spec| spec.id == model)
                .and_then(|spec| spec.efforts.first().cloned());
            if let Some(fallback) = fallback {
                warnings.push(format!(
                    "effort `{effort}` is not offered for `{model}`; using `{fallback}`"
                ));
                effort = fallback;
            }
        }

        provider
            .validate_selection(&model, &effort)
            .map_err(HarnessError::Other)?;

        let ledger = self
            .ledger
            .unwrap_or_else(|| Arc::new(Mutex::new(Ledger::load())));

        let provider_owns_agent_loop = provider.owns_agent_loop();

        // One condition, read twice: whether the ACP tool gets registered, and
        // whether the prompt is allowed to talk about it. Computing it in two
        // places would eventually let them disagree, and the failure mode is a
        // prompt describing a tool the model cannot see.
        let is_parent = self.role == RuntimeRole::Parent;
        let external_delegate_enabled = is_parent
            && self.enable_external_agents
            && !provider_owns_agent_loop
            && !config.agents.is_empty();

        let mut base_system = if provider_owns_agent_loop {
            match self.system {
                // The CLI and desktop deliberately pass the normal Zest base
                // prompt into every runtime. It describes Zest's local tools,
                // which Claude Code owns itself, so replace that exact default
                // with the parent-provider guidance.
                Some(system) if system == DEFAULT_SYSTEM => {
                    crate::prompt::CLAUDE_CODE_PARENT_SYSTEM.to_string()
                }
                Some(system) => system,
                None => crate::prompt::CLAUDE_CODE_PARENT_SYSTEM.to_string(),
            }
        } else {
            self.system.unwrap_or_else(|| DEFAULT_SYSTEM.to_string())
        };
        if external_delegate_enabled {
            base_system.push_str("\n\n");
            base_system.push_str(crate::prompt::EXTERNAL_DELEGATION_SYSTEM);
        }
        if is_parent && self.questioner.is_some() && !provider_owns_agent_loop {
            base_system.push_str("\n\n");
            base_system.push_str(crate::prompt::INTERACTIVE_QUESTION_SYSTEM);
        }
        if is_parent && self.browser.is_some() && !provider_owns_agent_loop {
            base_system.push_str("\n\n");
            base_system.push_str(LOCAL_BROWSER_SYSTEM);
        }
        let custom = load_custom_system(&root).map_err(HarnessError::Other)?;
        let project_docs = load_project_docs(&root);
        let skills = Arc::new(RwLock::new(SkillSet::discover()));
        let system = {
            let guard = skills
                .read()
                .map_err(|_| HarnessError::Other("skill registry lock poisoned".into()))?;
            let composed = compose_system_with_docs(&base_system, &custom, &project_docs, &guard);
            // Environment goes after the cache breakpoint, not merely last in
            // the string. Concatenating the two put the branch name inside the
            // cached block, so checking out a branch and reopening the project
            // threw away the base prompt, project docs, and every skill
            // description to re-report one line.
            SystemPrompt::new(composed).with_volatile(env_context(&root))
        };

        let mut worker_tools = ToolRegistry::new();
        if !provider_owns_agent_loop {
            register_read_tools(&mut worker_tools, &root)
                .map_err(|e| HarnessError::Other(format!("register read tools: {e}")))?;
            if self.register_write && self.role != RuntimeRole::DelegationReviewer {
                register_write_tools(&mut worker_tools, &root)
                    .map_err(|e| HarnessError::Other(format!("register write tools: {e}")))?;
            }
            register_skill_tools(&mut worker_tools, skills.clone());
        }

        // `bash` is deliberately *not* in `worker_tools`. A delegated worker
        // runs on a different provider to think about something; letting it
        // also run shell commands widens the blast radius for no benefit that
        // the parent conversation cannot already provide.
        let mut tools = worker_tools.clone();
        if !provider_owns_agent_loop && is_parent {
            if let Some(browser) = self.browser {
                register_browser_tool(&mut tools, browser);
            }
        }
        if is_parent && !provider_owns_agent_loop && self.register_exec && config.tools.bash.enabled
        {
            register_exec_tools(&mut tools, &root, config.tools.bash.settings())
                .map_err(|e| HarnessError::Other(format!("register exec tools: {e}")))?;
        }

        // Zest-owned MCP servers, and only for a provider whose agent loop Zest
        // owns. A Claude Code or Codex parent already loads MCP servers from
        // its own configuration, so registering Zest's on top would give that
        // chat two MCP stacks and one of them outside its permission prompts.
        // Registration reads the cached catalogue rather than handshaking with
        // every server, so a configured-but-unreachable server cannot hold up
        // the first message.
        if is_parent && !provider_owns_agent_loop && !config.mcp.is_empty() {
            let uncatalogued = register_mcp_tools(
                &mut tools,
                &config.mcp,
                &crate::mcp::McpCatalog::load(),
                &root,
            );
            if !uncatalogued.is_empty() {
                // Saying nothing here is the bad outcome: the server is
                // configured and switched on, so the user has every reason to
                // expect its tools, and would otherwise only learn they are
                // missing from the model failing to use them.
                warnings.push(format!(
                    "No tools loaded for the MCP server{} {}. Check {} in Customize → MCPs.",
                    if uncatalogued.len() == 1 { "" } else { "s" },
                    uncatalogued.join(", "),
                    if uncatalogued.len() == 1 {
                        "it"
                    } else {
                        "them"
                    }
                ));
            }
        }

        let registry = Arc::new(registry);

        // Cloned before the delegation block below moves the field.
        let spill_thread_id = self.parent_thread_id.clone();

        if external_delegate_enabled {
            tools.register(Arc::new(ExternalAgent::with_parent_secret_envs(
                &root,
                config.agents.clone(),
                config.provider_key_env_names(),
            )));
            tools.register(Arc::new(FeatureDelegator::new(
                &root,
                config.agents.clone(),
                self.parent_thread_id
                    .unwrap_or_else(|| "coordinator".to_string()),
            )));
        }
        if is_parent && self.questioner.is_some() && !provider_owns_agent_loop {
            register_question_tool(&mut tools);
        }

        // Oversized results go to `.zest/spill/<chat-id>/` and the model gets a
        // locator instead of losing the bytes. A front-end that supplied no
        // thread gets a synthetic id: the artifacts are then only reachable
        // within this process, and the store's own sibling sweep collects them
        // later, since no thread deletion will ever name them.
        if config.tools.max_result_bytes > 0 {
            let id = spill_thread_id.unwrap_or_else(|| crate::thread::new_id("session"));
            match SpillStore::open(&root, &id) {
                Ok(store) => {
                    let policy = SpillPolicy::new(store, config.tools.max_result_bytes);
                    tools = tools.with_spill(Arc::new(policy));
                }
                Err(e) => warnings.push(format!("oversized tool results will stay inline: {e}")),
            }
        }

        let approver = if self.role == RuntimeRole::DelegationWorker {
            Arc::new(AllowApprover) as Arc<dyn Approver>
        } else {
            self.approver
                .unwrap_or_else(|| Arc::new(DenyApprover) as Arc<dyn Approver>)
        };
        let questioner = self
            .questioner
            .unwrap_or_else(|| Arc::new(DenyQuestioner) as Arc<dyn Questioner>);
        let policy = if self.role == RuntimeRole::DelegationWorker {
            Arc::new(Mutex::new(ApprovalPolicy::new(ApprovalMode::Bypass)))
        } else {
            self.policy
                .unwrap_or_else(|| Arc::new(Mutex::new(ApprovalPolicy::default())))
        };

        let mut agent = Agent::new(provider, tools)
            .with_system(system)
            .with_ledger(ledger.clone())
            .with_approver(approver)
            .with_questioner(questioner)
            .with_policy(policy.clone());
        agent.model = model.clone();
        agent.effort = effort.clone();

        Ok(RuntimeSession {
            root,
            config,
            registry,
            provider_id,
            model,
            effort,
            agent,
            ledger,
            policy,
            skills,
            base_system,
            warnings,
        })
    }
}

impl RuntimeSession {
    /// Resolve workspace root for callers that only have a path hint.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Resolve a native worker target without fallback to another provider/model.
pub fn resolve_provider_target(
    registry: &ProviderRegistry,
    skipped: &[Skipped],
    provider_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<ResolvedProviderTarget> {
    let provider = registry.get(provider_id).ok_or_else(|| {
        if let Some(entry) = skipped.iter().find(|entry| entry.id == provider_id) {
            HarnessError::Other(format!(
                "provider `{provider_id}` is unavailable: {}. Connect it or choose another configured provider.",
                entry.reason
            ))
        } else {
            HarnessError::Other(format!(
                "provider `{provider_id}` is not configured. Connect it or choose another configured provider."
            ))
        }
    })?;
    if provider.owns_agent_loop() {
        return Err(HarnessError::Other(format!(
            "provider `{provider_id}` owns its agent loop and cannot run as a native delegated worker"
        )));
    }
    let model = model
        .map(str::to_owned)
        .unwrap_or_else(|| provider.default_model().to_string());
    let effort = normalize_effort(effort.unwrap_or("high"));
    provider
        .validate_selection(&model, &effort)
        .map_err(HarnessError::Other)?;
    if let Some(spec) = provider.models().iter().find(|spec| spec.id == model) {
        if !spec.supports_tools {
            return Err(HarnessError::Other(format!(
                "model `{model}` on provider `{provider_id}` does not support tools and cannot run a delegated worker"
            )));
        }
    }
    Ok(ResolvedProviderTarget {
        provider_id: provider_id.to_string(),
        model,
        effort,
        provider,
    })
}

/// Explain why a selected provider is not available, and what to do about it.
fn unloadable_provider(
    provider_id: &str,
    config: &Config,
    skipped: &[Skipped],
    root: &Path,
) -> String {
    // The registry tried and failed — it knows exactly why (usually a missing
    // key env var), so quote it rather than paraphrase.
    if let Some(entry) = skipped.iter().find(|s| s.id == provider_id) {
        return format!(
            "provider `{provider_id}` is configured but could not be loaded: {}",
            entry.reason
        );
    }

    // Not in the config at all. Almost always means this folder has no
    // zest.toml and there is no user-global one either.
    let user_path = crate::config::user_config_path()
        .map(|p| crate::fsutil::display_path(&p))
        .unwrap_or_else(|| "~/.zest/zest.toml".to_string());
    let known: Vec<&str> = config.providers.keys().map(String::as_str).collect();
    let available = if known.is_empty() {
        "none are configured".to_string()
    } else {
        format!("configured here: {}", known.join(", "))
    };

    format!(
        "provider `{provider_id}` is not configured for {} ({available}). \
         Add a zest.toml to that folder, or create {user_path} so your providers \
         apply to every project.",
        crate::fsutil::display_path(root)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct StubBrowser;

    #[async_trait::async_trait]
    impl crate::tools::BrowserAdapter for StubBrowser {
        async fn execute(
            &self,
            _request: crate::tools::BrowserRequest,
        ) -> std::result::Result<serde_json::Value, String> {
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zest-runtime-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn interactive_question_guidance_requires_an_interactive_front_end() {
        let dir = two_provider_dir("question-runtime");
        let config = Config::find(&dir).unwrap();

        let headless = RuntimeBuilder::new(&dir)
            .with_config(config.clone())
            .with_provider("codex")
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert!(!headless.agent.tool_names().contains(&"ask_user"));
        assert!(!headless.agent.system_text().contains("# Asking the user"));

        let interactive = RuntimeBuilder::new(&dir)
            .with_config(config)
            .with_provider("codex")
            .with_questioner(Arc::new(DenyQuestioner))
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert!(interactive.agent.tool_names().contains(&"ask_user"));
        assert!(interactive
            .agent
            .system_text()
            .contains("# Asking the user"));
    }

    /// The environment block names the git branch, so it differs between two
    /// sessions in one project. Keeping it out of the cacheable half is what
    /// lets the base prompt, project docs, and skills survive a checkout.
    #[test]
    fn the_environment_block_is_kept_out_of_the_cacheable_prompt() {
        let dir = two_provider_dir("system-split");
        let runtime = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("codex")
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();

        let system = runtime.agent.system.as_ref().expect("a composed prompt");
        assert!(
            !system.cacheable.contains("# Environment"),
            "the cached half must not carry the environment: {}",
            system.cacheable
        );
        assert!(system.volatile.contains("# Environment"));
        // The model still reads both, in the same order as before the split.
        assert!(system.text().contains("# Environment"));
        assert!(runtime.agent.system_text().contains("Working directory:"));
    }

    #[test]
    fn browser_is_parent_runtime_only_and_adds_its_guidance() {
        let dir = two_provider_dir("browser-runtime");
        let runtime = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("codex")
            .with_browser_adapter(Arc::new(StubBrowser))
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();

        assert!(runtime.agent.tool_names().contains(&"browser"));
        assert!(runtime.agent.system_text().contains("# Local browser"));
    }

    #[test]
    fn external_agents_are_explicit_tools_and_follow_the_external_worker_switch() {
        let dir = scratch("external-agent");
        let config = Config::parse(
            r#"
[providers.codex]
kind = "openai_compatible"
base_url = "http://127.0.0.1:1/v1"
model = "gpt-5.6-sol"

[agents.review]
mode = "headless"
command = "review-agent"
args = ["--output-format", "stream-json", "{prompt}"]
workspace = "isolated"

[default]
provider = "codex"
"#,
        )
        .unwrap();

        let enabled = RuntimeBuilder::new(&dir)
            .with_config(config.clone())
            .with_provider("codex")
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert!(enabled.agent.tool_names().contains(&"delegate_external"));
        assert!(enabled.agent.tool_names().contains(&"delegate_feature"));
        assert!(enabled.agent.system_text().contains("# External workers"));

        let disabled = RuntimeBuilder::new(&dir)
            .with_config(config)
            .with_provider("codex")
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert!(!disabled.agent.tool_names().contains(&"delegate_external"));
    }

    #[test]
    fn claude_code_parent_owns_tools_and_cannot_delegate() {
        let dir = scratch("claude-code-parent");
        let config = Config::parse(
            r#"
[providers.claude]
kind = "claude_code"
model = "sonnet"
permission_mode = "accept_edits"

[agents.review]
mode = "headless"
command = "review-agent"
args = ["--output-format", "stream-json", "{prompt}"]
workspace = "isolated"

[default]
provider = "claude"
"#,
        )
        .unwrap();

        let runtime = RuntimeBuilder::new(&dir)
            .with_config(config)
            .with_provider("claude")
            .with_system(DEFAULT_SYSTEM)
            .register_write_tools(true)
            .register_exec_tools(true)
            .enable_external_agents(true)
            .build()
            .unwrap();

        assert!(runtime.agent.tool_names().is_empty());
        assert!(runtime.agent.system_text().contains("parent coding agent"));
        assert!(!runtime
            .agent
            .system_text()
            .contains("File tools are scoped"));
        assert!(!runtime.agent.system_text().contains("# External workers"));
    }

    /// Reproduces the reported failure: opening a folder that has no
    /// `zest.toml` while `codex` is the selected provider.
    #[test]
    fn opening_a_folder_with_no_config_names_the_real_problem() {
        // Canonicalized, because that is what the desktop passes in and it is
        // where the `\\?\` prefix comes from on Windows.
        let dir = std::fs::canonicalize(scratch("no-config")).unwrap();
        // Guard against the assertion below quietly becoming vacuous if
        // canonicalize ever stops producing the prefix.
        #[cfg(windows)]
        assert!(
            dir.display().to_string().starts_with(r"\\?\"),
            "test no longer exercises the prefix it is checking for"
        );
        let config = Config::env_fallback();
        let (_, skipped) = ProviderRegistry::from_config(&config);

        let message = unloadable_provider("codex", &config, &skipped, &dir);

        // The old message claimed codex was "configured", which was false and
        // sent you looking for a key that was never the problem.
        assert!(
            !message.contains("is configured but"),
            "must not claim it was configured: {message}"
        );
        assert!(message.contains("not configured for"), "{message}");
        assert!(message.contains("zest.toml"), "{message}");
        // Says where to put a config that survives switching projects.
        assert!(message.contains(".zest"), "{message}");
        // A raw `\\?\` extended-length path in user-facing copy reads as
        // corruption. canonicalize() produces them, so this is easy to reintroduce.
        assert!(
            !message.contains(r"\\?\"),
            "extended-length prefix leaked into the message: {message}"
        );
    }

    #[test]
    fn a_configured_provider_missing_its_key_quotes_the_reason() {
        let dir = scratch("missing-key");
        std::env::remove_var("ZEST_TEST_UNLOADABLE_KEY");
        let config = Config::parse(
            r#"
[providers.codex]
kind = "anthropic"
api_key_env = "ZEST_TEST_UNLOADABLE_KEY"
"#,
        )
        .unwrap();
        let (_, skipped) = ProviderRegistry::from_config(&config);

        let message = unloadable_provider("codex", &config, &skipped, &dir);
        assert!(message.contains("is configured but"), "{message}");
        // Naming the variable is the whole point — it is the fix.
        assert!(message.contains("ZEST_TEST_UNLOADABLE_KEY"), "{message}");
    }

    #[test]
    fn user_config_is_used_when_the_project_has_none() {
        // Providers follow the machine, not the repository: opening an
        // unrelated folder must not lose your Codex login.
        let project = scratch("bare-project");
        let home = scratch("fake-home");
        let user_dir = home.join(".zest");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(
            user_dir.join("zest.toml"),
            r#"
[providers.codex]
kind = "codex_cli"
model = "gpt-5.6-sol"
"#,
        )
        .unwrap();

        // `Config::find` consults the real home dir, so exercise the layering
        // through the pieces it composes rather than by moving the user's home.
        assert!(!project.join("zest.toml").is_file());
        let user = Config::load_from(user_dir.join("zest.toml")).unwrap();
        assert!(user.providers.contains_key("codex"));
        let (registry, skipped) = ProviderRegistry::from_config(&user);
        assert!(
            registry.get("codex").is_some(),
            "gateway with no api_key_env needs no key: {skipped:?}"
        );
    }

    #[test]
    fn project_config_still_wins_over_user_config() {
        let dir = scratch("project-wins");
        std::fs::write(
            dir.join("zest.toml"),
            r#"
[providers.local]
kind = "openai_compatible"
base_url = "http://127.0.0.1:11434/v1"
model = "llama"
"#,
        )
        .unwrap();
        let config = Config::find(&dir).unwrap();
        assert!(config.providers.contains_key("local"));
        // Whatever is in ~/.zest/zest.toml must not leak in beside it —
        // a merged provider table makes "which account pays" ambiguous.
        assert_eq!(config.providers.len(), 1);
    }

    /// Two providers with disjoint model catalogues, both loadable.
    ///
    /// `openai_compatible` on purpose: it is the only remaining kind that loads
    /// without a key, carries an explicit model allow-list, *and* leaves Zest
    /// owning the agent loop. The two subscription kinds own their own loop, so a
    /// fixture built on them would correctly register no tools at all.
    fn two_provider_dir(name: &str) -> PathBuf {
        let dir = scratch(name);
        let mut f = std::fs::File::create(dir.join("zest.toml")).unwrap();
        writeln!(
            f,
            r#"
[providers.codex]
kind = "openai_compatible"
base_url = "http://127.0.0.1:8317/v1"
model = "gpt-5.6-sol"

[providers.claude]
kind = "openai_compatible"
base_url = "http://127.0.0.1:8317/v1"
model = "claude-opus-5"
models = ["claude-opus-5", "claude-sonnet-5"]

[default]
provider = "codex"
model = "gpt-5.6-sol"
"#
        )
        .unwrap();
        dir
    }

    /// The reported failure: a Codex model left in the Claude slot by an old
    /// preference file made Claude impossible to select — and the only way to
    /// change the model was to start a session that could no longer start.
    #[test]
    fn a_stale_remembered_model_is_dropped_not_fatal() {
        let dir = two_provider_dir("stale-model");
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("claude")
            .with_remembered_options(Some("gpt-5.6-luna".into()), Some("medium".into()))
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .expect("a stale preference must not strand the provider");

        assert_eq!(session.model, "claude-opus-5", "fell back to the default");
        assert!(
            session.warnings.iter().any(|w| w.contains("gpt-5.6-luna")),
            "the drop must be reported: {:?}",
            session.warnings
        );
    }

    /// The soft landing must not extend to an effort the caller asked for —
    /// `alpha_prove` relies on that rejection.
    #[test]
    fn a_stale_remembered_effort_falls_back_but_an_explicit_one_errors() {
        let dir = scratch("effort-split");
        let mut f = std::fs::File::create(dir.join("zest.toml")).unwrap();
        writeln!(
            f,
            r#"
[providers.codex]
kind = "codex_cli"
model = "gpt-a"
efforts = ["low", "high"]

[default]
provider = "codex"
model = "gpt-a"
"#
        )
        .unwrap();

        // Remembered: dropped, with a warning.
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_remembered_options(None, Some("max".into()))
            .enable_external_agents(false)
            .register_exec_tools(false)
            .build()
            .expect("a stale effort must not strand the provider");
        assert_eq!(session.effort, "low");
        assert!(session.warnings.iter().any(|w| w.contains("max")));

        // Explicit: still an error.
        let explicit = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_effort("max")
            .enable_external_agents(false)
            .register_exec_tools(false)
            .build();
        assert!(explicit.is_err(), "explicit effort must not be swallowed");
    }

    #[test]
    fn a_valid_remembered_model_is_still_honoured() {
        let dir = two_provider_dir("good-model");
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("claude")
            .with_remembered_options(Some("claude-sonnet-5".into()), None)
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert_eq!(session.model, "claude-sonnet-5");
        assert!(session.warnings.is_empty(), "{:?}", session.warnings);
    }

    #[test]
    fn a_thread_scoped_spill_directory_reaches_the_agent() {
        let dir = two_provider_dir("spill-wiring");
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("claude")
            .with_parent_thread_id("t-abc")
            .enable_external_agents(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert_eq!(session.agent.spill_dir(), Some(".zest/spill/t-abc"));
        assert!(session.warnings.is_empty(), "{:?}", session.warnings);
        // Building a runtime must not litter: the directory appears on the first
        // result that actually spills, not before.
        assert!(!dir.join(".zest/spill").exists());
    }

    #[test]
    fn a_zero_result_cap_leaves_the_agent_without_a_spill_store() {
        let dir = two_provider_dir("spill-off");
        let mut config = Config::find(&dir).unwrap();
        config.tools.max_result_bytes = 0;
        let session = RuntimeBuilder::new(&dir)
            .with_config(config)
            .with_provider("claude")
            .with_parent_thread_id("t-abc")
            .enable_external_agents(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        assert_eq!(session.agent.spill_dir(), None);
    }

    /// A front-end with no durable thread still gets bounding; the id is
    /// synthetic and the artifacts are collected by the store's sibling sweep.
    #[test]
    fn a_runtime_without_a_thread_still_bounds_results() {
        let dir = two_provider_dir("spill-anon");
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("claude")
            .enable_external_agents(false)
            .register_exec_tools(false)
            .build()
            .unwrap();
        let spill_dir = session
            .agent
            .spill_dir()
            .expect("a store should be attached");
        assert!(spill_dir.starts_with(".zest/spill/session-"), "{spill_dir}");
        assert!(session.warnings.is_empty(), "{:?}", session.warnings);
    }

    /// The opposite treatment: something the user just picked must fail loudly
    /// rather than silently becoming a different model.
    #[test]
    fn an_explicitly_chosen_bad_model_still_errors() {
        let dir = two_provider_dir("explicit-bad");
        let built = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("claude")
            .with_model("gpt-5.6-luna")
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build();
        let err = match built {
            Ok(session) => panic!("expected a rejection, got model {}", session.model),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("gpt-5.6-luna"), "{err}");
        assert!(err.contains("not supported"), "{err}");
    }

    /// `ZEST_MODEL` is global and cannot know which provider it lands on, so it
    /// must not strand one either.
    #[test]
    fn zest_model_env_is_ignored_for_a_provider_that_lacks_it() {
        let dir = two_provider_dir("env-model");
        std::env::set_var("ZEST_MODEL", "gpt-5.6-luna");
        let built = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("claude")
            .enable_external_agents(false)
            .register_write_tools(false)
            .register_exec_tools(false)
            .build();
        std::env::remove_var("ZEST_MODEL");
        assert_eq!(built.expect("must not strand").model, "claude-opus-5");
    }

    #[test]
    fn a_config_whose_only_provider_lacks_its_key_fails_the_build() {
        let dir = scratch("cfg");
        let mut f = std::fs::File::create(dir.join("zest.toml")).unwrap();
        writeln!(
            f,
            r#"
[providers.main]
kind = "anthropic"
api_key_env = "ZEST_TEST_RUNTIME_ABSENT_KEY"

[default]
provider = "main"
"#
        )
        .unwrap();
        std::env::remove_var("ZEST_TEST_RUNTIME_ABSENT_KEY");

        let err = match RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .build()
        {
            Ok(_) => panic!("expected build to fail without a key"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("could not be loaded") || msg.contains("unavailable"),
            "{msg}"
        );
    }

    #[test]
    fn delegated_worker_profile_has_read_write_skills_only() {
        let dir = two_provider_dir("worker-profile");
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("codex")
            .with_role(RuntimeRole::DelegationWorker)
            .build()
            .unwrap();
        let names = session.agent.tool_names();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"read_skill"));
        assert!(!names.contains(&"bash"));
        assert!(!names.contains(&"delegate_external"));
        assert!(!names.contains(&"ask_user"));
    }

    #[test]
    fn delegated_reviewer_profile_is_read_only() {
        let dir = two_provider_dir("reviewer-profile");
        let session = RuntimeBuilder::new(&dir)
            .with_config(Config::find(&dir).unwrap())
            .with_provider("codex")
            .with_role(RuntimeRole::DelegationReviewer)
            .build()
            .unwrap();
        let names = session.agent.tool_names();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"read_skill"));
        assert!(!names.contains(&"write_file"));
        assert!(!names.contains(&"bash"));
        assert!(!names.contains(&"delegate_external"));
    }

    #[test]
    fn provider_owned_loop_is_rejected_for_native_worker() {
        let dir = scratch("owned-worker");
        let config = Config::parse(
            r#"
[providers.claude]
kind = "claude_code"

[default]
provider = "claude"
"#,
        )
        .unwrap();
        let result = RuntimeBuilder::new(&dir)
            .with_config(config)
            .with_provider("claude")
            .with_role(RuntimeRole::DelegationWorker)
            .build();
        let error = match result {
            Ok(_) => panic!("provider-owned loop must not be accepted as a native worker"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("owns its agent loop"));
    }
}
