//! Native chat transport. The GPUI thread only sends commands. Zest's agent owns execution.
use crate::attachments::{self, AttachmentInput};
use crate::bridge::{Bridge, Cmd, Ev, EvKind};
use crate::interaction::Interactions;
use futures::channel::mpsc::{self, UnboundedSender};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use zest_core::*;

pub fn executor() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("native runtime")
    })
}

#[derive(Clone)]
pub struct Project {
    pub path: PathBuf,
    pub registry_dir: PathBuf,
    pub config_root: PathBuf,
    pub store: Arc<ThreadStore>,
    pub config: Config,
    pub ledger: Arc<Mutex<Ledger>>,
    pub jobs: Arc<JobRegistry>,
    pub delegations: Arc<zest_coordinator::DelegationCoordinator>,
    pub board_events: tokio::sync::watch::Receiver<()>,
    pub skills: Arc<SkillSet>,
    pub mcp: Vec<zest_core::McpSlash>,
    pub demo: bool,
}

impl Project {
    /// Reloads `zest.toml` and the MCP slash commands after a settings page edited them.
    /// Open chats keep the configuration they started with.
    pub fn reload_config(&mut self) -> Result<()> {
        if self.demo {
            return Ok(());
        }
        self.config = Config::find(&self.config_root)?;
        self.mcp = mcp_slashes(&self.config.mcp, &McpCatalog::load());
        Ok(())
    }

    pub fn open(path: PathBuf, demo: bool) -> Result<Self> {
        let registry_dir = if demo {
            path.join(".zest/demo-desktop")
        } else {
            dirs::config_dir().ok_or_else(|| error("Cannot find Zest configuration directory"))?.join("zest")
        };
        Self::open_with_registry(path, demo, registry_dir)
    }

    pub(super) fn open_with_registry(path: PathBuf, demo: bool, registry_dir: PathBuf) -> Result<Self> {
        let path = std::fs::canonicalize(path).map_err(error)?;
        if !path.is_dir() {
            return Err(HarnessError::Other("Choose a project directory".into()));
        }
        let config_root = zest_core::worktree_registry::config_root(&registry_dir, &path).map_err(error)?;
        let config = if demo {
            Config::parse("")?
        } else {
            Config::find(&config_root)?
        };
        static LIVE_LEDGER: OnceLock<Arc<Mutex<Ledger>>> = OnceLock::new();
        let ledger = if demo {
            Arc::new(Mutex::new(Ledger::load_from(
                path.join(".zest/demo-usage.json"),
            )))
        } else {
            LIVE_LEDGER
                .get_or_init(|| Arc::new(Mutex::new(Ledger::load())))
                .clone()
        };
        let mcp = if demo {
            Vec::new()
        } else {
            mcp_slashes(&config.mcp, &McpCatalog::load())
        };
        let skills = if demo {
            SkillSet::default()
        } else {
            SkillSet::discover()
        };
        let (notifier, board_events) = tokio::sync::watch::channel(());
        let delegations = zest_coordinator::DelegationCoordinator::with_runtime(
            ledger.clone(),
            Arc::new(zest_coordinator::TokioSpawner::from_handle(executor().handle().clone())),
            Arc::new(BoardNotifier(notifier)),
        ).with_lock_retention(zest_coordinator::LockRetention::WhileActive);
        Ok(Self {
            delegations: Arc::new(delegations),
            board_events,
            store: Arc::new(ThreadStore::open(&path)?),
            path,
            registry_dir,
            config_root,
            config,
            mcp,
            ledger,
            jobs: Arc::new(JobRegistry::new()),
            skills: Arc::new(skills),
            demo,
        })
    }

    pub fn default_provider(&self) -> String {
        if self.demo {
            return "demo".into();
        }
        self.config
            .default_target()
            .map(|t| t.provider)
            .or_else(|| self.config.providers.keys().next().cloned())
            .unwrap_or_default()
    }

    pub fn list_commands(&self) -> Vec<zest_core::commands::SlashCommand> {
        zest_core::commands::list_slash_commands(&self.skills, &self.mcp)
    }
}

struct BoardNotifier(tokio::sync::watch::Sender<()>);
impl zest_coordinator::DelegationNotifier for BoardNotifier {
    fn notify(&self, _: zest_coordinator::DelegationEvent) {
        self.0.send_replace(());
    }
}

#[derive(Clone, Default)]
pub struct Selection {
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone)]
pub struct Chat {
    browser: Arc<crate::web_content::Browser>,
    pub project: Project,
    pub thread: Arc<Mutex<Thread>>,
    pub options: Arc<Mutex<Selection>>,
    pub policy: Arc<Mutex<ApprovalPolicy>>,
    pub context_meter: Arc<Mutex<Option<crate::context_meter::ContextUsageView>>>,
}

fn error(message: impl ToString) -> HarnessError {
    HarnessError::Other(message.to_string())
}

pub fn provider_failure(err: HarnessError) -> HarnessError {
    match err {
        HarnessError::Cancelled => err,
        other => error(other.turn_message()),
    }
}

impl Chat {
    pub fn new(project: Project, thread: Thread) -> Self {
        let provider = thread
            .provider_id
            .clone()
            .unwrap_or_else(|| project.default_provider());
        let target = project
            .config
            .default_target()
            .filter(|t| t.provider == provider);
        let prefs = ProjectSessionState::load(&project.path, &provider).get(&provider);
        let options = Selection {
            provider,
            model: prefs
                .model
                .or_else(|| target.as_ref().and_then(|t| t.model.clone())),
            effort: prefs.effort.or_else(|| target.and_then(|t| t.effort)),
        };
        Self {
            project,
            thread: Arc::new(Mutex::new(thread)),
            browser: Arc::default(),
            options: Arc::new(Mutex::new(options)),
            policy: Arc::new(Mutex::new(ApprovalPolicy::new(ApprovalMode::Manual))),
            context_meter: Arc::new(Mutex::new(None)),
        }
    }

    pub fn current_context_usage(&self) -> Option<crate::context_meter::ContextUsageView> {
        if let Ok(guard) = self.context_meter.lock()
            && let Some(meter) = guard.as_ref()
        {
            return Some(meter.clone());
        }
        let (tx, _) = futures::channel::mpsc::unbounded();
        let (agent, _, _) = self.agent(&tx).ok()?;
        let checkpoints = self.thread.lock().ok().map_or(0, |t| t.checkpoints.len());
        let meter = crate::context_meter::estimate_context(&agent, checkpoints);
        if let Ok(mut guard) = self.context_meter.lock() {
            *guard = Some(meter.clone());
        }
        Some(meter)
    }

    // Commit the projection only after the atomic file replacement succeeds.
    pub fn edit<T>(&self, change: impl FnOnce(&mut Thread) -> Result<T>) -> Result<T> {
        let mut live = self.thread.lock().map_err(error)?;
        let mut next = live.clone();
        let value = change(&mut next)?;
        self.project.store.save(&next)?;
        *live = next;
        Ok(value)
    }

    /// Restores the conversation to a checkpoint in place: later messages and checkpoints are
    /// discarded and the provider session is dropped. Files the agent changed are not touched.
    /// Returns the checkpoint's label.
    pub fn rewind(&self, checkpoint_id: &str) -> Result<String> {
        let mut live = self.thread.lock().map_err(error)?;
        let label = live
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == checkpoint_id)
            .map(|checkpoint| checkpoint.label.clone())
            .ok_or_else(|| HarnessError::Other("That checkpoint is not part of this chat.".into()))?;
        *live = self.project.store.rewind_to_checkpoint(&live, checkpoint_id)?;
        Ok(label)
    }

    /// Takes a sent user message back so it can be edited: the conversation returns to just
    /// before it (the message and everything after it are discarded, like a rewind), and the
    /// message's text is returned for the composer. Files the agent changed are not touched.
    pub fn edit_message(&self, message_id: &str) -> Result<String> {
        let mut live = self.thread.lock().map_err(error)?;
        let text = live
            .messages
            .iter()
            .find_map(|message| match message {
                StoredMessage::User { id, text, .. } if id == message_id => Some(text.clone()),
                _ => None,
            })
            .ok_or_else(|| HarnessError::Other("Only your own messages can be edited.".into()))?;
        *live = self.project.store.rewind_before_user_message(&live, message_id)?;
        Ok(text)
    }

    pub fn enqueue(&self, text: &str, files: Vec<ThreadInputAttachment>) -> Result<ThreadInput> {
        self.edit(|t| {
            t.enqueue_input(ThreadInputTarget::Followup, text, files)
                .map_err(error)
        })
    }

    pub fn bridge(&self) -> Bridge {
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (events, rx) = mpsc::unbounded();
        let chat = self.clone();
        executor().spawn(async move {
            let mut cancel = CancelToken::new();
            let mut running: Option<tokio::task::JoinHandle<()>> = None;
            while let Some(command) = commands.recv().await {
                match command {
                    Cmd::Run(_) | Cmd::Compact => {
                        // The UI releases its reservation only on End, the worker's final send.
                        if let Some(previous) = running.take() {
                            let _ = previous.await;
                        }
                        cancel = CancelToken::new();
                        let (chat, events, token) = (chat.clone(), events.clone(), cancel.clone());
                        running = Some(tokio::spawn(async move {
                            let result = match command {
                                Cmd::Run(id) => chat.run(&id, &events, &token).await,
                                Cmd::Compact => chat.compact(&events, &token).await,
                                _ => unreachable!(),
                            };
                            let failed = result.is_err();
                            if let Err(e) = result {
                                emit(&events, EvKind::Failure(e.to_string()));
                            }
                            emit(
                                &events,
                                EvKind::End {
                                    tokens: 0,
                                    cancelled: failed || token.is_cancelled(),
                                },
                            );
                        }));
                    }
                    Cmd::Cancel => cancel.cancel(),
                    _ => {}
                }
            }
            cancel.cancel();
        });
        Bridge { tx, rx: Some(rx) }
    }

    pub fn agent(&self, events: &UnboundedSender<Ev>) -> Result<(Agent, String, &'static str)> {
        let options = self.options.lock().map_err(error)?.clone();
        let host = Arc::new(Interactions {
            events: events.clone(),
            policy: self.policy.clone(),
        });
        let thread_id = self.thread.lock().map_err(error)?.id.clone();
        let (agent, kind) = if self.project.demo {
            let mut tools = ToolRegistry::new();
            register_write_tools(&mut tools, &self.project.path).map_err(error)?;
            register_question_tool(&mut tools);
            (
                Agent::new(Arc::new(Demo), tools)
                    .with_approver(host.clone())
                    .with_questioner(host.clone())
                    .with_policy(self.policy.clone()),
                "demo",
            )
        } else {
            let cfg = self
                .project
                .config
                .providers
                .get(&options.provider)
                .ok_or_else(|| {
                    error(format!(
                        "Choose a configured provider. '{}' is unavailable.",
                        options.provider
                    ))
                })?;
            let mut builder = RuntimeBuilder::new(&self.project.path)
                .with_config(self.project.config.clone())
                .with_provider(&options.provider)
                .with_approver(host.clone())
                .with_questioner(host.clone())
                .with_ledger(self.project.ledger.clone())
                .with_job_registry(self.project.jobs.clone())
                .with_browser_adapter(self.browser.clone())
                .with_html_artifacts(true)
                .with_policy(self.policy.clone())
                .with_job_owner(&thread_id)
                .with_parent_thread_id(&thread_id);
            if let Some(model) = &options.model {
                builder = builder.with_model(model);
            }
            if let Some(effort) = &options.effort {
                builder = builder.with_effort(effort);
            }
            let session = builder.build()?;
            for warning in session.warnings {
                emit(events, EvKind::Notice(warning));
            }
            (session.agent, cfg.kind_tag())
        };
        let t = self.thread.lock().map_err(error)?;
        t.assert_provider(&options.provider).map_err(error)?;
        t.assert_provider_kind(kind).map_err(error)?;
        Ok((
            agent
                .with_messages(t.agent_messages.clone())
                .with_provider_session(t.provider_session.clone())
                .with_provider_interaction(host),
            options.provider,
            kind,
        ))
    }

    async fn compact(&self, events: &UnboundedSender<Ev>, cancel: &CancelToken) -> Result<()> {
        let (mut agent, _, _) = self.agent(events)?;
        self.compact_agent(&mut agent, events, cancel).await
    }

    async fn compact_agent(
        &self,
        agent: &mut Agent,
        events: &UnboundedSender<Ev>,
        cancel: &CancelToken,
    ) -> Result<()> {
        let result = tokio::select! {
            res = agent.compact_context() => res.map_err(provider_failure)?,
            _ = cancel.cancelled() => return Err(HarnessError::Cancelled),
        };
        self.edit(|t| {
            self.project.store.create_checkpoint_with_metadata(
                t,
                "Before compaction",
                None,
                None,
                ThreadCheckpointKind::Compaction,
            )?;
            t.set_agent_messages(agent.messages_for_persist());
            t.provider_session = agent.provider_session();
            Ok(())
        })?;
        let message = match result {
            zest_core::agent::CompactionOutcome::Pruned { results_pruned, .. } => {
                format!("Compacted {results_pruned} tool results.")
            }
            zest_core::agent::CompactionOutcome::Summarized { .. } => {
                "Compacted conversation context.".into()
            }
        };
        emit(events, EvKind::Notice(message));
        Ok(())
    }

    async fn run(
        &self,
        input_id: &str,
        events: &UnboundedSender<Ev>,
        cancel: &CancelToken,
    ) -> Result<()> {
        let (mut agent, provider, kind) = self.agent(events)?;
        if !self.project.demo && let Ok(Some(context)) = tokio::task::spawn_blocking(crate::plugins::agent_context).await {
            let system = agent.system.get_or_insert_with(|| SystemPrompt::new(DEFAULT_SYSTEM));
            if !system.volatile.is_empty() { system.volatile.push_str("\n\n"); }
            system.volatile.push_str("# Enabled local integrations\n\n");
            system.volatile.push_str(&context);
        }
        let assistant = new_id("assistant");
        let user = new_id("user");
        let run = new_id("turn");
        let lifecycle = ChatPersistence::open(&self.project.path)?;
        let content = self.edit(|t| {
            t.ensure_provider(&provider).map_err(error)?;
            t.ensure_provider_kind(kind).map_err(error)?;
            let input = t
                .claim_input(input_id)
                .ok_or_else(|| error("Queued message no longer exists"))?;
            let files: Vec<_> = input.attachments.into_iter().map(to_attachment).collect();
            let mut expanded = expand_command(&input.text, &self.project.skills, &self.project.mcp);
            if expanded.command.is_none()
                && self.policy.lock().map_err(error)?.mode() == ApprovalMode::Plan
            {
                expanded = expand_command_as(&input.text, &self.project.skills, "plan");
            }
            let blocks = attachments::build_user_content(&expanded.prompt, &files);
            if blocks.is_empty() {
                return Err(error("Message has no usable content"));
            }
            let display = attachments::format_display_message(&input.text, &files);
            self.project.store.create_checkpoint_with_metadata(
                t,
                "Before turn",
                Some(user.clone()),
                Some(truncate_chars(&display, 100)),
                ThreadCheckpointKind::Turn,
            )?;
            t.apply_user(&user, &display);
            t.apply_assistant_start(&assistant, expanded.command.as_deref());
            t.record_event(ThreadEventKind::TurnStarted {
                turn_id: run.clone(),
            });
            Ok(blocks)
        })?;
        let recorded = match lifecycle.runs.create_or_resume_for_turn(
            &run,
            &self.thread.lock().map_err(error)?.id,
            &provider,
            &user,
            &assistant,
        ) {
            Ok(_) => true,
            Err(e) => {
                emit(
                    events,
                    EvKind::Notice(format!("Run history unavailable: {e}")),
                );
                false
            }
        };
        agent.set_next_run_id(run.clone());
        emit(events, EvKind::Start);
        emit(
            events,
            EvKind::SessionInfo(format!(
                "{}  ·  {}  ·  {}",
                provider, agent.model, agent.effort
            )),
        );
        let mut last_save = Instant::now();
        let mut write_failed = false;
        let mut has_text = false;
        let mut round_break = false;
        let mut on_event = |event: StreamEvent<'_>| {
            if let StreamEvent::ToolCallResult {
                metadata: Some(ToolMetadata::Delegation { job_id: Some(id), .. }), ..
            } = &event {
                if let Err(e) = self.project.delegations.apply_dispatch_receipt(&self.project.path, id) {
                    emit(events, EvKind::Notice(e));
                }
            }
            if let StreamEvent::ResumeHandle(handle) = event {
                if recorded && let Err(e) = lifecycle.runs.set_resume_handle(&run, handle) {
                    emit(events, EvKind::Notice(e.to_string()));
                }
                return;
            }
            let mut t = self.thread.lock().expect("thread");
            match &event {
                StreamEvent::ToolCallStart { .. } if has_text => round_break = true,
                StreamEvent::Text(s) if !s.trim().is_empty() => {
                    if round_break && !s.starts_with('\n') {
                        t.apply_text_delta(&assistant, "\n\n");
                        emit(events, EvKind::Text("\n\n".into()));
                    }
                    has_text = true;
                    round_break = false;
                }
                _ => {}
            }
            if let Some(ui) = apply(&mut t, &assistant, event) {
                emit(events, ui);
            }
            if last_save.elapsed() >= Duration::from_millis(250) {
                if let Err(e) = self.project.store.save(&t) {
                    if !write_failed {
                        emit(events, EvKind::Notice(e.to_string()));
                    }
                    write_failed = true;
                }
                last_save = Instant::now();
            }
        };
        let result = agent
            .send_blocks_cancellable(content, &mut on_event, Some(cancel))
            .await
            .map_err(provider_failure);
        let checkpoints = {
            let mut t = self.thread.lock().map_err(error)?;
            match &result {
                Ok(()) => {
                    t.apply_done(&assistant);
                    t.set_agent_messages(agent.messages_for_persist());
                    t.provider_session = agent.provider_session();
                }
                Err(e) => {
                    t.terminalize_interrupted();
                    t.apply_error(&assistant, &e.to_string());
                    // The agent committed the completed part of the turn, closed with an
                    // interruption note: tools may already have changed files, so the next turn
                    // must see it. A provider session cannot resume a broken turn, so drop it.
                    t.set_agent_messages(agent.messages_for_persist());
                    t.provider_session = None;
                }
            }
            let status = if cancel.is_cancelled() {
                "cancelled"
            } else if result.is_ok() {
                "done"
            } else {
                "error"
            };
            t.record_event(ThreadEventKind::TurnCompleted {
                turn_id: run.clone(),
                status: status.into(),
            });
            if let Err(e) = self.project.store.save(&t) {
                let _ = lifecycle
                    .runs
                    .mark_failed(&run, "Final transcript could not be saved");
                return Err(e);
            }
            let marked = if !recorded {
                Ok(())
            } else {
                match &result {
                    Ok(()) => lifecycle
                        .runs
                        .mark_completed(&run, agent.last_usage.as_ref()),
                    Err(HarnessError::Cancelled) => lifecycle.runs.mark_aborted(&run),
                    Err(e) => lifecycle.runs.mark_failed(&run, e.to_string()),
                }
            };
            if let Err(e) = marked {
                emit(
                    events,
                    EvKind::Notice(format!("Could not finish run record: {e}")),
                );
            }
            t.checkpoints.len()
        };
        let mut meter = crate::context_meter::estimate_context(&agent, checkpoints);
        if result.is_ok() && meter.should_auto_compact && !cancel.is_cancelled() {
            emit(events, EvKind::Notice("Compacting context…".into()));
            match self.compact_agent(&mut agent, events, cancel).await {
                Ok(()) => meter = crate::context_meter::estimate_context(&agent, checkpoints + 1),
                Err(e) => emit(events, EvKind::Notice(format!("Compaction: {e}"))),
            }
        }
        if let Ok(mut guard) = self.context_meter.lock() {
            *guard = Some(meter.clone());
        }
        let source = if meter.source == "estimate" {
            "estimated"
        } else {
            "measured"
        };
        emit(
            events,
            EvKind::SessionInfo(format!(
                "{}  ·  {}  ·  {:.0}% context  ·  {} tokens left ({source})",
                provider, agent.model, meter.percent_full, meter.remaining_tokens
            )),
        );
        result
    }
}

fn emit(events: &UnboundedSender<Ev>, kind: EvKind) {
    let _ = events.unbounded_send(Ev {
        at: Instant::now(),
        kind,
    });
}

fn apply(t: &mut Thread, msg: &str, ev: StreamEvent<'_>) -> Option<EvKind> {
    Some(match ev {
        StreamEvent::Text(s) => {
            t.apply_text_delta(msg, s);
            EvKind::Text(s.into())
        }
        StreamEvent::Thinking(s) => {
            t.apply_thinking_delta(msg, s);
            EvKind::Thinking(s.into())
        }
        StreamEvent::ToolCallStart { name, id } => {
            t.apply_tool_start(msg, id, name);
            EvKind::ToolStart {
                id: id.into(),
                name: name.into(),
            }
        }
        StreamEvent::ToolCallResult {
            name,
            id,
            summary,
            is_error,
            path,
            diff,
            metadata,
        } => {
            let artifact = crate::artifact::metadata_document(metadata.as_ref());
            t.apply_tool_result(msg, id, name, summary, is_error, path, diff, metadata);
            EvKind::ToolResult {
                artifact,
                id: id.into(),
                summary: summary.into(),
                diff: diff.map(str::to_owned),
                is_error,
            }
        }
        StreamEvent::ProviderActivity { id, title, status } => {
            t.apply_provider_activity(msg, id, title, status, None);
            EvKind::Activity {
                id: id.into(),
                title: title.into(),
                status: status.into(),
            }
        }
        StreamEvent::ApprovalNeeded {
            approval_id,
            tool_call_id,
            tool_name,
            path,
            summary,
            diff,
            ..
        } => {
            t.apply_approval_needed(
                msg,
                &tool_call_id,
                &tool_name,
                &approval_id,
                &path,
                &summary,
                &diff,
            );
            return None;
        }
        StreamEvent::ModelSubstituted { requested, served } => {
            EvKind::Notice(format!("Requested {requested}, provider served {served}"))
        }
        _ => return None,
    })
}

pub fn to_attachment(f: ThreadInputAttachment) -> AttachmentInput {
    AttachmentInput {
        name: f.name,
        detail: f.detail,
        content: f.content,
        status: f.status,
        kind: f.kind,
        media_type: f.media_type,
        data_base64: f.data_base64,
    }
}

pub fn prepared(f: attachments::PreparedAttachment) -> ThreadInputAttachment {
    ThreadInputAttachment {
        name: f.name,
        detail: f.detail,
        content: f.content,
        status: f.status,
        kind: Some(f.kind),
        media_type: f.media_type,
        data_base64: f.data_base64,
    }
}

/// Offline mode exercises the same Agent, history, event, attachment and persistence path.
struct Demo;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_provider_leaves_the_input_queued_for_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::open(dir.path().into(), true).unwrap();
        project.demo = false;
        let chat = Chat::new(project, Thread::new());
        chat.options.lock().unwrap().provider = "missing-provider".into();
        let input = chat.enqueue("Keep my message", vec![]).unwrap();
        let (events, _rx) = mpsc::unbounded();
        let result = executor().block_on(chat.run(&input.id, &events, &CancelToken::new()));
        assert!(result.unwrap_err().to_string().contains("Choose a configured provider"));
        let id = chat.thread.lock().unwrap().id.clone();
        let saved = chat.project.store.load(&id).unwrap();
        assert_eq!(saved.pending_inputs[0].id, input.id);
        assert!(saved.messages.is_empty());
        assert!(saved.agent_messages.is_empty());
    }

    #[test]
    fn rewind_restores_a_checkpoint_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path().into(), true).unwrap();
        let chat = Chat::new(project, Thread::new());
        let (events, _rx) = mpsc::unbounded();
        for text in ["First question", "Second question"] {
            let input = chat.enqueue(text, vec![]).unwrap();
            executor()
                .block_on(chat.run(&input.id, &events, &CancelToken::new()))
                .unwrap();
        }
        let (before_second, kept) = {
            let t = chat.thread.lock().unwrap();
            assert_eq!(t.checkpoints.len(), 2, "one checkpoint before each turn");
            (t.checkpoints[1].id.clone(), t.checkpoints[1].message_count)
        };

        assert_eq!(chat.rewind(&before_second).unwrap(), "Before turn");

        let id = chat.thread.lock().unwrap().id.clone();
        let saved = chat.project.store.load(&id).unwrap();
        assert_eq!(saved.messages.len(), kept, "the second turn is gone");
        assert_eq!(saved.messages[0].searchable_text(), "First question");
        assert_eq!(saved.checkpoints.len(), 2, "checkpoints up to the restored one stay");
        assert_eq!(saved.provider_session, None);
        assert_eq!(chat.thread.lock().unwrap().messages.len(), kept, "the open chat matches");
        assert!(chat.rewind("not-a-checkpoint").is_err());
    }

    #[test]
    fn editing_a_message_returns_to_just_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path().into(), true).unwrap();
        let chat = Chat::new(project, Thread::new());
        let (events, _rx) = mpsc::unbounded();
        for text in ["First question", "Second question"] {
            let input = chat.enqueue(text, vec![]).unwrap();
            executor()
                .block_on(chat.run(&input.id, &events, &CancelToken::new()))
                .unwrap();
        }
        let user_ids: Vec<String> = chat
            .thread
            .lock()
            .unwrap()
            .messages
            .iter()
            .filter_map(|m| match m {
                StoredMessage::User { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(user_ids.len(), 2);

        assert_eq!(chat.edit_message(&user_ids[1]).unwrap(), "Second question");
        let id = chat.thread.lock().unwrap().id.clone();
        let saved = chat.project.store.load(&id).unwrap();
        assert_eq!(saved.messages.len(), 2, "only the first turn is left");
        assert_eq!(saved.messages[0].searchable_text(), "First question");
        assert_eq!(saved.provider_session, None);

        assert_eq!(chat.edit_message(&user_ids[0]).unwrap(), "First question");
        assert!(chat.thread.lock().unwrap().messages.is_empty(), "editing the first message empties the chat");
        assert!(chat.edit_message("not-a-message").is_err());
    }

    #[test]
    fn a_stopped_turn_keeps_its_interrupted_history() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path().into(), true).unwrap();
        let chat = Chat::new(project, Thread::new());
        chat.thread.lock().unwrap().provider_session =
            Some(zest_core::provider::ProviderSessionRef::CursorAcp { session_id: "stale".into() });
        let input = chat.enqueue("Explain the stop button", vec![]).unwrap();
        let (events, _rx) = mpsc::unbounded();
        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        // The demo provider streams a word every 35 ms, so this stops it partway through.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            stopper.cancel();
        });
        let result = executor().block_on(chat.run(&input.id, &events, &cancel));
        assert!(matches!(result, Err(HarnessError::Cancelled)), "{result:?}");

        let id = chat.thread.lock().unwrap().id.clone();
        let saved = chat.project.store.load(&id).unwrap();
        let wire = serde_json::to_string(&saved.agent_messages).unwrap();
        assert!(wire.contains("Explain the stop button"), "the user message is kept: {wire}");
        assert!(wire.contains("this turn was interrupted"), "the interruption note is kept: {wire}");
        assert_eq!(saved.provider_session, None, "a provider session cannot resume a broken turn");
    }

    #[test]
    fn provider_transport_details_stay_out_of_chat_errors() {
        let err = HarnessError::Stream {
            kind: "internal".into(),
            message: "private transport diagnostic".into(),
        };
        assert!(!provider_failure(err).to_string().contains("private transport diagnostic"));
        assert!(matches!(provider_failure(HarnessError::Cancelled), HarnessError::Cancelled));
    }
}
#[async_trait::async_trait]
impl Provider for Demo {
    fn id(&self) -> &str {
        "demo"
    }
    fn default_model(&self) -> &str {
        "offline"
    }
    fn auth_status(&self) -> auth::AuthStatus {
        auth::AuthStatus::Ready { account: None }
    }
    async fn stream_turn(
        &self,
        req: &TurnRequest,
        f: &mut (dyn for<'a> FnMut(StreamEvent<'a>) + Send),
    ) -> Result<Completion> {
        let prompt = req
            .messages
            .last()
            .and_then(|m| m.content.first())
            .and_then(|b| b.get("text"))
            .and_then(|s| s.as_str());
        let tool = match prompt {
            Some("Demo: write") => Some((
                "write_file",
                serde_json::json!({"path":"demo.txt","content":"Approved in GPUI"}),
            )),
            Some("Demo: question") => Some((
                "ask_user",
                serde_json::json!({"question":"Which file?","choices":["First","Second"]}),
            )),
            _ => None,
        };
        if let Some((name, input)) = tool.filter(|_| req.allow_tool_use) {
            let id = new_id("demo-tool");
            return Ok(Completion {
                content: vec![
                    serde_json::json!({"type":"tool_use","id":id,"name":name,"input":input}),
                ],
                stop_reason: Some("tool_use".into()),
                usage: Default::default(),
                usage_available: false,
                limits: None,
                served_model: None,
                provider_session: None,
            });
        }
        let answer = format!(
            "Offline reply. This conversation has {} wire messages.",
            req.messages.len()
        );
        f(StreamEvent::Thinking("Checking conversation history."));
        for word in answer.split_inclusive(' ') {
            f(StreamEvent::Text(word));
            tokio::time::sleep(Duration::from_millis(35)).await;
        }
        Ok(Completion {
            content: vec![serde_json::json!({"type":"text", "text":answer})],
            stop_reason: Some("end_turn".into()),
            usage: Default::default(),
            usage_available: false,
            limits: None,
            served_model: None,
            provider_session: None,
        })
    }
}
