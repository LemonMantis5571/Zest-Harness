use super::*;
use crate::native::{Chat, Project};
use crate::native_view::button;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use zest_core::{ApprovalMode, Thread, ThreadSummary};
mod board;
#[path = "workspace_test.rs"]
mod workspace_test;

actions!(
    native_workspace,
    [NewChat, FindChat, OpenFolder, ToggleSplit, ClosePanel]
);

#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Chat,
    Search,
    Settings,
    Commands,
    Files,
    Changes,
    Usage,
    Jobs,
    Delegation,
    Checkpoints,
    Rename,
}

pub struct Workspace {
    project: Project,
    board: board::Board,
    chats: HashMap<String, Entity<Root>>,
    draft_ids: Vec<String>,
    active: String,
    second: Option<String>,
    history: Arc<Vec<ThreadSummary>>,
    // Edits that landed after the background directory scan started.
    loading_history: Option<HashMap<String, ThreadSummary>>,
    panel: Panel,
    filter: Entity<Composer>,
    hits: Vec<search::Hit>,
    search_term: String,
    search_task: Option<Task<()>>,
    search_cancel: zest_core::CancelToken,
    focus: FocusHandle,
    note: String,
    report: String,
    report_epoch: usize,
    login: Option<zest_core::auth::LoginProcess>,
    directory: String,
    entries: Vec<workspace_files::WorkspaceFileView>,
    preview: Option<workspace_files::WorkspaceFileContent>,
}

const PAGES: &[(Panel, &str)] = &[
    (Panel::Search, "Find chats"),
    (Panel::Settings, "Model & permissions"),
    (Panel::Commands, "Skills & MCP"),
    (Panel::Files, "Files"),
    (Panel::Changes, "Changes"),
    (Panel::Usage, "Usage"),
    (Panel::Jobs, "Jobs"),
    (Panel::Delegation, "Delegation"),
    (Panel::Checkpoints, "Checkpoints"),
];

impl Workspace {
    fn new(project: Project, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(Composer::new);
        cx.observe(&filter, |this, _, cx| {
            this.search(cx);
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &filter,
            window,
            |this, _, ev: &ComposerEvent, window, cx| {
                if let ComposerEvent::Send { text, .. } = ev {
                    match this.panel {
                        Panel::Rename => {
                            let result = this.chat(cx).edit(|t| {
                                t.set_title(text).map_err(zest_core::HarnessError::Other)
                            });
                            match result {
                                Ok(_) => this.panel = Panel::Chat,
                                Err(e) => this.note = e.to_string(),
                            }
                            this.refresh(cx);
                        }
                        Panel::Search => {
                            let term = text.to_lowercase();
                            if let Some(row) = this
                                .history
                                .iter()
                                .find(|r| chat_title(r).to_lowercase().contains(&term))
                            {
                                this.open(&row.id.clone(), window, cx);
                            } else if let Some(hit) = this
                                .hits
                                .first()
                                .cloned()
                                .filter(|_| this.search_term == *text)
                            {
                                this.open_hit(&hit, window, cx);
                            } else if let Some(&(page, _)) = PAGES.iter().find(|(p, name)| {
                                *p != Panel::Search && name.to_lowercase().contains(&term)
                            }) {
                                this.page(page, window, cx);
                            } else {
                                this.filter.update(cx, |field, cx| {
                                    field.text = text.clone();
                                    field.sel = text.len()..text.len();
                                    cx.notify();
                                });
                            }
                        }
                        _ => {}
                    }
                }
            },
        )
        .detach();
        let board = board::Board::new(&project, cx);
        let mut ws = Self {
            board,
            project,
            chats: HashMap::new(),
            draft_ids: vec![],
            active: String::new(),
            second: None,
            history: Arc::default(),
            loading_history: None,
            panel: Panel::Chat,
            filter,
            focus: cx.focus_handle(),
            note: String::new(),
            report: String::new(),
            report_epoch: 0,
            login: None,
            directory: String::new(),
            entries: vec![],
            preview: None,
            hits: vec![],
            search_term: String::new(),
            search_task: None,
            search_cancel: zest_core::CancelToken::new(),
        };
        ws.add(Thread::new(), window, cx);
        ws.load_history(cx);
        ws
    }

    fn chat(&self, cx: &App) -> Chat {
        self.chats[&self.active]
            .read(cx)
            .native
            .as_ref()
            .unwrap()
            .clone()
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.loading_history.is_some() {
            return;
        }
        self.loading_history = Some(HashMap::new());
        let store = self.project.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.list() })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let updates = ws.loading_history.take().unwrap_or_default();
                match result {
                    Ok(rows) => {
                        ws.history = Arc::new(merge_history(rows, updates));
                        ws.draft_ids
                            .retain(|id| !ws.history.iter().any(|row| row.id == *id));
                        ws.refresh_search(cx);
                    }
                    Err(e) => ws.note = e.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let chat = self.chat(cx);
        self.update_history(chat.thread.lock().unwrap().summary(), cx);
    }

    fn update_history(&mut self, summary: ThreadSummary, cx: &mut Context<Self>) {
        if summary.message_count == 0
            && summary.pending_input_count == 0
            && !self.history.iter().any(|row| row.id == summary.id)
        {
            return;
        }
        if let Some(updates) = &mut self.loading_history {
            updates.insert(summary.id.clone(), summary.clone());
        }
        self.draft_ids.retain(|id| id != &summary.id);
        let rows = Arc::make_mut(&mut self.history);
        rows.retain(|row| row.id != summary.id);
        let index = rows.partition_point(|row| history_order(row, &summary).is_lt());
        rows.insert(index, summary);
        self.refresh_search(cx);
        cx.notify();
    }

    fn refresh_search(&mut self, cx: &mut Context<Self>) {
        if self.panel == Panel::Search {
            self.search_term.clear();
            self.hits.clear();
            self.search(cx);
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        if self.panel != Panel::Search {
            return;
        }
        let term = self.filter.read(cx).text.trim().to_string();
        if term == self.search_term {
            return;
        }
        self.search_term = term.clone();
        self.hits.clear();
        self.search_task = None;
        self.search_cancel.cancel();
        if term.is_empty() {
            return;
        }
        self.search_cancel = zest_core::CancelToken::new();
        let cancel = self.search_cancel.clone();
        let store = self.project.store.clone();
        let history = self.history.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let query = term.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { search::search(&store, &history, &query, &cancel) })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if ws.search_term == term && ws.panel == Panel::Search {
                    ws.hits = hits;
                    cx.notify();
                }
            });
        }));
    }

    fn open_hit(&mut self, hit: &search::Hit, window: &mut Window, cx: &mut Context<Self>) {
        self.open(&hit.thread, window, cx);
        if self.active != hit.thread {
            return;
        }
        self.chats[&self.active].update(cx, |r, cx| {
            if r.reveal_message(&hit.message) {
                cx.notify();
            }
        });
    }

    fn add(&mut self, thread: Thread, window: &mut Window, cx: &mut Context<Self>) {
        let id = thread.id.clone();
        if thread.messages.is_empty()
            && thread.pending_inputs.is_empty()
            && !self.history.iter().any(|row| row.id == id)
            && !self.draft_ids.contains(&id)
        {
            self.draft_ids.push(id.clone());
        }
        let native = Chat::new(self.project.clone(), thread);
        let view = cx.new(|cx| Root::open_chat(native, window, cx));
        cx.subscribe(&view, |this, view, _: &ChatChanged, cx| {
            let summary = view
                .read(cx)
                .native
                .as_ref()
                .unwrap()
                .thread
                .lock()
                .unwrap()
                .summary();
            this.update_history(summary, cx);
        })
        .detach();
        let owner = id.clone();
        cx.subscribe(&view, move |ws, _, _: &native_view::ChooseModel, cx| {
            if ws.second.as_deref() == Some(&owner) {
                ws.second = Some(ws.active.clone());
            }
            ws.active = owner.clone();
            ws.panel = Panel::Settings;
            cx.notify();
        })
        .detach();
        window.focus(&view.read(cx).composer.read(cx).focus.clone(), cx);
        self.chats.insert(id.clone(), view);
        self.active = id;
        self.panel = Panel::Chat;
        cx.notify();
    }

    fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if id != self.active && self.chats[&self.active].read(cx).side.open {
            self.chats[&self.active].update(cx, |r, cx| r.close_side(cx));
        }
        if self.second.as_deref() == Some(id) {
            self.second = Some(self.active.clone());
        }
        if let Some(chat) = self.chats.get(id) {
            self.active = id.into();
            self.panel = Panel::Chat;
            window.focus(&chat.read(cx).composer.read(cx).focus.clone(), cx);
        } else {
            match self.project.store.load_with_recovery(id) {
                Ok(loaded) => {
                    self.note = loaded.warning.unwrap_or_default();
                    self.add(loaded.thread, window, cx);
                }
                Err(e) => self.note = e.to_string(),
            }
        }
        cx.notify();
    }

    fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.second.take().is_none() {
            let previous = self.active.clone();
            self.add(Thread::new(), window, cx);
            self.second = Some(previous);
        }
        self.panel = Panel::Chat;
        cx.notify();
    }

    fn fork(&mut self, checkpoint: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let chat = self.chat(cx);
        if self.chats[&self.active].read(cx).turn_active {
            return;
        }
        let thread = chat.thread.lock().unwrap();
        let fork = match checkpoint {
            Some(id) => self.project.store.fork_from_checkpoint(&thread, id, None),
            None => self.project.store.fork(&thread, None),
        };
        match fork {
            Ok(child) => {
                self.add(child, window, cx);
                self.refresh(cx);
            }
            Err(e) => self.note = e.to_string(),
        }
        cx.notify();
    }

    fn page(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = panel;
        self.report_epoch += 1;
        self.report.clear();
        match panel {
            Panel::Rename | Panel::Search => {
                if panel == Panel::Search {
                    self.load_history(cx);
                }
                let initial = if panel == Panel::Rename {
                    self.chat(cx)
                        .thread
                        .lock()
                        .unwrap()
                        .title
                        .clone()
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                self.filter.update(cx, |field, cx| {
                    field.placeholder = if panel == Panel::Rename {
                        "Chat title"
                    } else {
                        "Search chats and commands"
                    };
                    field.text = initial;
                    field.sel = 0..field.text.len();
                    field.marked = None;
                    field.reversed = false;
                    cx.notify();
                });
                window.focus(&self.filter.read(cx).focus.clone(), cx);
            }
            Panel::Files => self.browse(self.directory.clone(), true, cx),
            Panel::Changes => {
                let epoch = self.report_epoch;
                let path = self.project.path.clone();
                let context = self
                    .chat(cx)
                    .thread
                    .lock()
                    .unwrap()
                    .git_context
                    .clone()
                    .unwrap_or_default();
                self.report = "Reading Git changes…".into();
                let task = native::executor().spawn(async move {
                    zest_core::workspace_changes::inspect(
                        path,
                        context.start_commit.as_deref(),
                        context.base_branch.as_deref(),
                    )
                    .await
                });
                cx.spawn(async move |this, cx| {
                    let text = match task.await {
                        Ok(Ok(s)) if s.unavailable => {
                            "Git changes unavailable for this folder.".to_string()
                        }
                        Ok(Ok(s)) if s.changed_files.is_empty() => {
                            "Working tree clean.".to_string()
                        }
                        Ok(Ok(s)) => format!(
                            "{} files  ·  +{}  −{}\n\n{}\n\n{}{}",
                            s.changed_files.len(),
                            s.additions,
                            s.deletions,
                            s.changed_files
                                .iter()
                                .map(|f| format!("{}  {}", f.status, f.path))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            s.diff,
                            if s.truncated {
                                "\n[Patch truncated]"
                            } else {
                                ""
                            }
                        ),
                        Ok(Err(e)) => e.to_string(),
                        Err(e) => e.to_string(),
                    };
                    let _ = this.update(cx, |ws, cx| {
                        if ws.report_epoch == epoch {
                            ws.report = text;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            Panel::Delegation => self.load_board(true, cx),
            Panel::Jobs => {
                let thread = &self.active;
                let lines: Vec<_> = self
                    .project
                    .jobs
                    .list(Some(thread))
                    .iter()
                    .map(|j| format!("{}  ·  {:?}  ·  {}", j.kind, j.status, j.label))
                    .collect();
                self.report = if lines.is_empty() {
                    "No background jobs for this chat.".into()
                } else {
                    lines.join("\n")
                };
            }
            Panel::Usage => {
                let snap = self.project.ledger.lock().unwrap().snapshot();
                self.report = snap
                    .providers
                    .iter()
                    .map(|p| {
                        format!(
                            "{}\n{} requests  ·  {} input tokens  ·  {} output tokens",
                            p.provider_id,
                            p.measured.requests,
                            p.measured.input_tokens,
                            p.measured.output_tokens
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if self.report.is_empty() {
                    self.report = "No measured usage yet.".into();
                }
            }
            Panel::Chat => window.focus(
                &self.chats[&self.active]
                    .read(cx)
                    .composer
                    .read(cx)
                    .focus
                    .clone(),
                cx,
            ),
            _ => {}
        }
        cx.notify();
    }

    fn select_model(
        &mut self,
        provider: &str,
        model: Option<String>,
        effort: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let chat = self.chat(cx);
        if self.chats[&self.active].read(cx).turn_active {
            return;
        }
        let owner = chat.thread.lock().unwrap().provider_id.clone();
        if owner.as_deref().is_some_and(|id| id != provider) {
            self.note = "Start a new chat to choose a different provider.".into();
        } else {
            let mut prefs = zest_core::ProjectSessionState::load(&self.project.path, provider);
            let saved = prefs.providers.entry(provider.to_owned()).or_default();
            saved.model = model.clone();
            saved.effort = effort.clone();
            if let Err(e) = prefs.save(&self.project.path) {
                self.note = e.to_string();
            }
            *chat.options.lock().unwrap() = native::Selection {
                provider: provider.into(),
                model,
                effort,
            };
            self.chats[&self.active].update(cx, |r, cx| {
                let options = chat.options.lock().unwrap();
                r.session_label = format!(
                    "{}  ·  {}  ·  {}",
                    options.provider,
                    options.model.as_deref().unwrap_or("default model"),
                    options.effort.as_deref().unwrap_or("default effort")
                );
                cx.notify();
            });
        }
        cx.notify();
    }

    fn panel_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let chat = self.chat(cx);
        let mut body = div()
            .id("page-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_2();
        match self.panel {
            Panel::Search => {
                let query = self.filter.read(cx).text.to_lowercase();
                body = body.child(self.filter.clone());
                for &(page, name) in PAGES
                    .iter()
                    .filter(|(p, name)| *p != Panel::Search && name.to_lowercase().contains(&query))
                {
                    body = body.child(button(
                        name,
                        name,
                        cx.listener(move |ws, _, w, cx| ws.page(page, w, cx)),
                    ));
                }
                for row in self
                    .history
                    .iter()
                    .filter(|s| chat_title(s).to_lowercase().contains(&query))
                {
                    let id = row.id.clone();
                    body = body.child(button(
                        SharedString::from(id.clone()),
                        chat_title(row),
                        cx.listener(move |ws, _, w, cx| ws.open(&id, w, cx)),
                    ));
                }
                for (i, hit) in self.hits.iter().enumerate() {
                    let hit = hit.clone();
                    body = body.child(button(
                        ("match", i),
                        format!("{}  ·  {}", hit.title, hit.excerpt),
                        cx.listener(move |ws, _, w, cx| ws.open_hit(&hit, w, cx)),
                    ));
                }
            }
            Panel::Rename => body = body.child(self.filter.clone()),
            Panel::Settings => {
                let selected = chat.options.lock().unwrap().clone();
                let busy = self.chats[&self.active].read(cx).turn_active;
                if busy {
                    body = body.child("Model changes are available when this turn finishes.");
                }
                let owner = chat.thread.lock().unwrap().provider_id.clone();
                body = body.child("Provider");
                let mut providers = div().flex().flex_wrap().gap_1();
                for id in self
                    .project
                    .config
                    .providers
                    .keys()
                    .filter(|id| owner.as_ref().is_none_or(|o| o == *id))
                {
                    let id = id.clone();
                    let label = mark(&id, id == selected.provider);
                    providers = providers.child(button(
                        SharedString::from(format!("provider-{id}")),
                        label,
                        cx.listener(move |ws, _, _, cx| ws.select_model(&id, None, None, cx)),
                    ));
                }
                body = body.child(providers);
                if let Some(cfg) = self.project.config.providers.get(&selected.provider) {
                    if self.login.is_some() {
                        body = body.child(button(
                            "cancel-login",
                            "Cancel sign-in",
                            cx.listener(|ws, _, _, cx| {
                                if let Some(mut login) = ws.login.take() {
                                    let _ = login.kill();
                                }
                                ws.note = "Sign-in cancelled.".into();
                                cx.notify();
                            }),
                        ));
                    } else if matches!(
                        cfg,
                        zest_core::ProviderConfig::CodexOAuth { .. }
                            | zest_core::ProviderConfig::CodexCli { .. }
                            | zest_core::ProviderConfig::ClaudeCode { .. }
                    ) {
                        body = body.child(button(
                            "login",
                            "Sign in",
                            cx.listener(|ws, _, _, cx| ws.start_login(cx)),
                        ));
                    }
                    let descriptor = zest_core::descriptor_from_config(&selected.provider, cfg);
                    let current = selected
                        .model
                        .as_deref()
                        .unwrap_or(&descriptor.default_model);
                    let mut models = div().flex().flex_wrap().gap_1();
                    for spec in &descriptor.models {
                        let (provider, model) = (selected.provider.clone(), spec.id.clone());
                        models = models.child(button(
                            SharedString::from(format!("model-{model}")),
                            mark(&model, current == model),
                            cx.listener(move |ws, _, _, cx| {
                                ws.select_model(&provider, Some(model.clone()), None, cx)
                            }),
                        ));
                    }
                    body = body.child("Model").child(models);
                    if let Some(spec) = descriptor.models.iter().find(|m| m.id == current) {
                        let mut efforts = div().flex().flex_wrap();
                        for e in &spec.efforts {
                            let (p, model, effort) =
                                (selected.provider.clone(), spec.id.clone(), e.clone());
                            efforts = efforts.child(button(
                                SharedString::from(format!("effort-{e}")),
                                mark(e, selected.effort.as_deref() == Some(e)),
                                cx.listener(move |ws, _, _, cx| {
                                    ws.select_model(
                                        &p,
                                        Some(model.clone()),
                                        Some(effort.clone()),
                                        cx,
                                    )
                                }),
                            ));
                        }
                        body = body.child("Reasoning effort").child(efforts);
                    }
                } else if self.project.demo {
                    body = body.child("Offline demo provider");
                } else {
                    body = body.child("Configure a provider in zest.toml to start a turn.");
                }
                let mode = chat.policy.lock().unwrap().mode();
                let mut modes = div().flex().flex_wrap();
                for m in [
                    ApprovalMode::Manual,
                    ApprovalMode::Plan,
                    ApprovalMode::AcceptEdits,
                    ApprovalMode::Auto,
                ] {
                    modes = modes.child(button(
                        m.as_str(),
                        mark(m.as_str(), mode == m),
                        cx.listener(move |ws, _, _, cx| {
                            ws.chat(cx).policy.lock().unwrap().set_mode(m);
                            cx.notify();
                        }),
                    ));
                }
                body = body.child("Tool permissions").child(modes);
            }
            Panel::Files => {
                if !self.directory.is_empty() || self.preview.is_some() {
                    body = body.child(button(
                        "up-dir",
                        "Up",
                        cx.listener(|ws, _, _, cx| {
                            let directory = if ws.preview.is_some() {
                                ws.directory.clone()
                            } else {
                                ws.directory
                                    .rsplit_once('/')
                                    .map(|p| p.0.to_owned())
                                    .unwrap_or_default()
                            };
                            ws.browse(directory, true, cx);
                        }),
                    ));
                }
                if let Some(file) = &self.preview {
                    body = body.child(file.path.clone()).child(
                        div()
                            .font_family(MONO)
                            .text_size(px(13.))
                            .child(file.content.clone()),
                    );
                    if file.truncated {
                        body = body.child(format!(
                            "Preview truncated ({} bytes on disk)",
                            file.byte_count
                        ));
                    }
                } else {
                    body = body.child(if self.directory.is_empty() {
                        "Project files".into()
                    } else {
                        self.directory.clone()
                    });
                    if !self.report.is_empty() {
                        body = body.child(self.report.clone());
                    }
                    for (index, entry) in self.entries.iter().enumerate() {
                        let path = entry.path.clone();
                        let folder = entry.kind == "directory";
                        let label = if folder {
                            format!("{}/", entry.name)
                        } else {
                            entry.name.clone()
                        };
                        body = body.child(button(
                            ("entry", index),
                            label,
                            cx.listener(move |ws, _, _, cx| ws.browse(path.clone(), folder, cx)),
                        ));
                    }
                }
            }
            Panel::Commands => {
                let commands =
                    zest_core::list_slash_commands(&self.project.skills, &self.project.mcp);
                if commands.is_empty() {
                    body = body.child("No skills or MCP commands configured.");
                }
                for (i, cmd) in commands.into_iter().enumerate() {
                    let name = cmd.name;
                    body = body.child(button(
                        ("slash", i),
                        format!("/{name}  ·  {}", cmd.description),
                        cx.listener(move |ws, _, w, cx| {
                            ws.page(Panel::Chat, w, cx);
                            let composer = ws.chats[&ws.active].read(cx).composer.clone();
                            composer.update(cx, |c, cx| {
                                c.replace_text_in_range(None, &format!("/{name} "), w, cx)
                            });
                        }),
                    ));
                }
            }
            Panel::Delegation => body = body.child(self.board_body(cx)),
            Panel::Checkpoints => {
                let t = chat.thread.lock().unwrap();
                if !self.chats[&self.active].read(cx).turn_active && !t.agent_messages.is_empty() {
                    body = body.child(button(
                        "compact",
                        "Compact context",
                        cx.listener(|ws, _, w, cx| {
                            ws.chats[&ws.active].update(cx, |r, cx| {
                                if !r.turn_active && r.cmd_tx.send(Cmd::Compact).is_ok() {
                                    r.turn_active = true;
                                    r.compacting = true;
                                    r.note = "Compacting context…".into();
                                    cx.notify();
                                }
                            });
                            ws.page(Panel::Chat, w, cx);
                        }),
                    ));
                }
                if t.checkpoints.is_empty() {
                    body = body.child("This chat has no checkpoints.");
                }
                for cp in t.checkpoints.iter().rev() {
                    let id = cp.id.clone();
                    body = body.child(button(
                        SharedString::from(id.clone()),
                        format!(
                            "Fork from {}  ·  {}",
                            cp.label,
                            cp.preview.as_deref().unwrap_or("")
                        ),
                        cx.listener(move |ws, _, w, cx| ws.fork(Some(&id), w, cx)),
                    ));
                }
            }
            _ => {
                if self.panel == Panel::Usage && !self.project.demo {
                    body = body.child(button(
                        "quota",
                        "Check provider quota",
                        cx.listener(|ws, _, _, cx| ws.check_quota(cx)),
                    ));
                }
                if !self.report.is_empty() {
                    body = body
                        .child(button(
                            "copy-report",
                            "Copy",
                            cx.listener(|ws, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(ws.report.clone()));
                            }),
                        ))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(13.))
                                .child(self.report.clone()),
                        );
                }
            }
        }
        body.into_any_element()
    }

    fn pick_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.into_iter().next() {
                    let result = cx.update(|cx| open_window(path, false, false, cx));
                    if let Err(e) = result {
                        let _ = this.update(cx, |ws, cx| {
                            ws.note = e;
                            cx.notify();
                        });
                    }
                }
            }
        })
        .detach();
    }

    fn browse(&mut self, path: String, folder: bool, cx: &mut Context<Self>) {
        self.report_epoch += 1;
        let epoch = self.report_epoch;
        let root = self.project.path.clone();
        self.entries.clear();
        self.preview = None;
        self.report = "Loading…".into();
        let destination = path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if folder {
                        workspace_files::list(&root, Some(&path)).map(|rows| (rows, None))
                    } else {
                        workspace_files::read(&root, &path).map(|file| (vec![], Some(file)))
                    }
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if ws.report_epoch != epoch {
                    return;
                }
                match result {
                    Ok((entries, preview)) => {
                        if folder {
                            ws.directory = destination;
                        }
                        ws.entries = entries;
                        ws.preview = preview;
                        ws.report = if folder && ws.entries.is_empty() {
                            "Empty directory".into()
                        } else {
                            String::new()
                        };
                    }
                    Err(e) => ws.report = e,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn start_login(&mut self, cx: &mut Context<Self>) {
        use zest_core::{ProviderConfig, auth};
        if self.login.is_some() {
            return;
        }
        let id = self.chat(cx).options.lock().unwrap().provider.clone();
        let result = match self.project.config.providers.get(&id) {
            Some(ProviderConfig::CodexOAuth { credential, .. }) => auth::start_codex_oauth_login(
                credential
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&id),
            ),
            Some(ProviderConfig::CodexCli { .. }) => auth::start_codex_cli_login(),
            Some(ProviderConfig::ClaudeCode { .. }) => auth::start_claude_code_login(),
            _ => return,
        };
        match result {
            Ok(login) => {
                self.note = login.spawn.browser_body.into();
                self.login = Some(login);
            }
            Err(e) => {
                self.note = e;
                cx.notify();
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let waiting = this.update(cx, |ws, cx| {
                    let Some(login) = ws.login.as_mut() else {
                        return false;
                    };
                    match login.poll_status() {
                        Ok(auth::LoginPoll::Running) => return true,
                        Ok(auth::LoginPoll::Succeeded) => ws.note = "Signed in.".into(),
                        Ok(auth::LoginPoll::Failed(detail)) => ws.note = detail,
                        Err(e) => ws.note = e.to_string(),
                    }
                    ws.login = None;
                    cx.notify();
                    false
                });
                if !waiting.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn check_quota(&mut self, cx: &mut Context<Self>) {
        let cfg = self.project.config.clone();
        self.report_epoch += 1;
        let epoch = self.report_epoch;
        self.report = "Checking provider quota…".into();
        let task =
            native::executor().spawn(async move { zest_core::fetch_provider_quotas(&cfg).await });
        cx.spawn(async move |this, cx| {
            let text = match task.await {
                Ok(snapshot) => snapshot
                    .providers
                    .into_iter()
                    .map(|provider| {
                        let mut lines =
                            vec![format!("{}  ·  {}", provider.provider_id, provider.detail)];
                        for b in provider.balances {
                            lines.push(format!("{} {} balance", b.total_balance, b.currency));
                        }
                        for w in provider.windows {
                            lines.push(format!("{}: {:.0}% used", w.label, w.used_percent));
                        }
                        lines.join("\n")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                Err(e) => e.to_string(),
            };
            let _ = this.update(cx, |ws, cx| {
                if ws.report_epoch == epoch {
                    ws.report = if text.is_empty() {
                        "No provider quota available.".into()
                    } else {
                        text
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Some(login) = &mut self.login {
            let _ = login.kill();
        }
    }
}

fn merge_history(
    mut rows: Vec<ThreadSummary>,
    updates: HashMap<String, ThreadSummary>,
) -> Vec<ThreadSummary> {
    rows.retain(|row| !updates.contains_key(&row.id));
    rows.extend(updates.into_values());
    rows.sort_by(history_order);
    rows
}

fn history_order(a: &ThreadSummary, b: &ThreadSummary) -> std::cmp::Ordering {
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| b.updated_at.cmp(&a.updated_at))
        .then_with(|| a.id.cmp(&b.id))
}

fn chat_title(row: &ThreadSummary) -> String {
    row.title.clone().unwrap_or_else(|| "Untitled chat".into())
}

#[cfg(test)]
mod history_tests {
    use super::{HashMap, Thread, merge_history};

    #[test]
    fn a_slow_scan_preserves_newer_edits_and_sorts_pinned_chats_first() {
        let mut a = Thread::new().summary();
        a.updated_at = 10;
        let mut b = Thread::new().summary();
        b.updated_at = 20;
        let mut pin = a.clone();
        pin.pinned = true;
        pin.title = Some("edited during scan".into());
        let mut c = Thread::new().summary();
        c.updated_at = 30;
        let rows = merge_history(
            vec![b.clone(), a.clone()],
            HashMap::from([(pin.id.clone(), pin), (c.id.clone(), c.clone())]),
        );
        assert_eq!(
            rows.iter().map(|r| &r.id).collect::<Vec<_>>(),
            [&a.id, &c.id, &b.id]
        );
        assert_eq!(rows[0].title.as_deref(), Some("edited during scan"));
    }
}
fn mark(s: &str, selected: bool) -> String {
    if selected {
        format!("✓ {s}")
    } else {
        s.into()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = self.chat(cx);
        let t = chat.thread.lock().unwrap();
        let title = t.title.clone().unwrap_or_else(|| "New chat".into());
        let pinned = t.pinned;
        let saved = !t.messages.is_empty();
        drop(t);
        let history = uniform_list(
            "history",
            self.draft_ids.len() + self.history.len(),
            cx.processor(|ws, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let (id, label) = if let Some(id) = ws.draft_ids.get(index) {
                            let chat = ws.chats.get(id)?.read(cx);
                            let line = first_line(&chat.composer.read(cx).text);
                            (
                                id,
                                if line.is_empty() {
                                    "New chat".into()
                                } else {
                                    format!("Draft · {line}")
                                },
                            )
                        } else {
                            let row = ws.history.get(index - ws.draft_ids.len())?;
                            let busy = ws
                                .chats
                                .get(&row.id)
                                .is_some_and(|c| c.read(cx).turn_active);
                            let prefix = if busy {
                                "Working · "
                            } else if row.pinned {
                                "Pinned · "
                            } else {
                                ""
                            };
                            (&row.id, format!("{prefix}{}", chat_title(row)))
                        };
                        let selected = &ws.active == id;
                        let id = id.clone();
                        Some(
                            div()
                                .h(px(28.))
                                .w_full()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .when(selected, |d| d.bg(rgb(0x293342)))
                                .rounded_md()
                                .child(button(
                                    SharedString::from(id.clone()),
                                    label,
                                    cx.listener(move |ws, _, w, cx| ws.open(&id, w, cx)),
                                )),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();
        let mut nav = div().flex().flex_col().gap_1();
        for &(panel, name) in PAGES {
            nav = nav.child(button(
                name,
                name,
                cx.listener(move |ws, _, w, cx| ws.page(panel, w, cx)),
            ));
        }
        let sidebar = div()
            .w(px(220.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(PANEL))
            .child(div().text_size(px(18.)).child("Zest"))
            .child(button(
                "project",
                self.project
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                cx.listener(|ws, _, _, cx| ws.pick_project(cx)),
            ))
            .child(button(
                "new",
                "+ New chat",
                cx.listener(|ws, _, w, cx| ws.add(Thread::new(), w, cx)),
            ))
            .child(history)
            .child(nav);
        let mut header = div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(if self.panel == Panel::Chat {
                        title
                    } else {
                        PAGES
                            .iter()
                            .find(|(p, _)| *p == self.panel)
                            .map(|(_, s)| s.to_string())
                            .unwrap_or_else(|| "Rename chat".into())
                    }),
            );
        if self.panel == Panel::Chat {
            if saved {
                header = header
                    .child(button(
                        "pin",
                        if pinned { "Unpin" } else { "Pin" },
                        cx.listener(|ws, _, _, cx| {
                            if let Err(e) = ws.chat(cx).edit(|t| Ok(t.set_pinned(!t.pinned))) {
                                ws.note = e.to_string();
                            }
                            ws.refresh(cx);
                        }),
                    ))
                    .child(button(
                        "rename",
                        "Rename",
                        cx.listener(|ws, _, w, cx| ws.page(Panel::Rename, w, cx)),
                    ))
                    .child(button(
                        "fork",
                        "Fork",
                        cx.listener(|ws, _, w, cx| ws.fork(None, w, cx)),
                    ));
            }
            header = header.child(button(
                "split",
                if self.second.is_some() {
                    "Single view"
                } else {
                    "Split"
                },
                cx.listener(|ws, _, w, cx| ws.split(w, cx)),
            ));
        } else {
            header = header.child(button(
                "back",
                "Back to chat",
                cx.listener(|ws, _, w, cx| ws.page(Panel::Chat, w, cx)),
            ));
        }
        let content = if self.panel == Panel::Chat {
            let mut panes = div().flex().flex_1().min_h_0();
            if let Some(id) = &self.second {
                let id = id.clone();
                let title = self.chats[&id]
                    .read(cx)
                    .native
                    .as_ref()
                    .unwrap()
                    .thread
                    .lock()
                    .unwrap()
                    .title
                    .clone()
                    .unwrap_or_else(|| "New chat".into());
                panes = panes.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(rgb(BORDER))
                        .child(button(
                            "select-pane",
                            format!("{title}  ·  Select pane"),
                            cx.listener(move |ws, _, w, cx| ws.open(&id, w, cx)),
                        ))
                        .child(self.chats[self.second.as_ref().unwrap()].clone()),
                );
            }
            panes
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.chats[&self.active].clone()),
                )
                .into_any_element()
        } else {
            self.panel_body(cx)
        };
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .on_action(cx.listener(|ws, _: &NewChat, w, cx| ws.add(Thread::new(), w, cx)))
            .on_action(cx.listener(|ws, _: &FindChat, w, cx| ws.page(Panel::Search, w, cx)))
            .on_action(cx.listener(|ws, _: &OpenFolder, _, cx| ws.pick_project(cx)))
            .on_action(cx.listener(|ws, _: &ToggleSplit, w, cx| ws.split(w, cx)))
            .on_action(cx.listener(|ws, _: &ClosePanel, w, cx| ws.page(Panel::Chat, w, cx)))
            .bg(rgb(BG))
            .font_family(SANS)
            .text_color(rgb(FG))
            .text_size(px(14.))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(header)
                    .when(!self.note.is_empty(), |d| {
                        d.child(div().p_2().child(self.note.clone()).child(button(
                            "dismiss",
                            "Dismiss",
                            cx.listener(|ws, _, _, cx| {
                                ws.note.clear();
                                cx.notify();
                            }),
                        )))
                    })
                    .child(content),
            )
    }
}

fn open_window(path: PathBuf, demo: bool, test: bool, cx: &mut App) -> Result<(), String> {
    let project = Project::open(path, demo).map_err(|e| e.to_string())?;
    for existing in cx.windows() {
        if let Some(handle) = existing.downcast::<Workspace>() {
            if handle
                .update(cx, |ws, window, cx| {
                    if ws.project.path != project.path {
                        return false;
                    }
                    window.activate_window();
                    cx.activate(true);
                    true
                })
                .unwrap_or(false)
            {
                return Ok(());
            }
        }
    }
    let bounds = Bounds::centered(None, size(px(1150.), px(790.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: !test,
                titlebar: Some(TitlebarOptions {
                    title: Some("Zest".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |w, cx| cx.new(|cx| Workspace::new(project, w, cx)),
        )
        .map_err(|e| e.to_string())?;
    if test {
        cx.spawn(async move |cx| {
            workspace_test::check(window, cx).await;
        })
        .detach();
    }
    Ok(())
}

pub fn launch_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    let mut path = None;
    let mut demo = false;
    let mut test = false;
    while let Some(arg) = args.next() {
        if arg == "--project" {
            path = Some(PathBuf::from(args.next().unwrap_or_else(|| {
                eprintln!("--project needs a directory");
                std::process::exit(2)
            })));
        } else if arg == "--demo" {
            demo = true;
        } else if arg == "--selftest-workspace" {
            demo = true;
            test = true;
        } else if arg == "--help" || arg == "-h" {
            println!(
                "Zest native workspace\n  --project DIRECTORY    Open a project (default: current directory)\n  --demo                 Open a temporary offline project\n  --selftest-workspace   Exercise the native workspace offline\n  --selftest-queue       Exercise the original stream fixture"
            );
            return true;
        } else if arg.to_str().is_some_and(|s| {
            matches!(
                s.split('=').next().unwrap_or(""),
                "--agent"
                    | "--bench"
                    | "--scroll"
                    | "--idle"
                    | "--selftest"
                    | "--selftest-queue"
                    | "--webview"
                    | "--history"
                    | "--rate"
                    | "--secs"
                    | "--langs"
                    | "--open"
                    | "--focus"
                    | "--topmost"
                    | "--show"
                    | "--a11y"
            )
        }) && path.is_none()
            && !demo
        {
            return false;
        } else {
            eprintln!(
                "Unknown or incompatible workspace option: {}",
                arg.to_string_lossy()
            );
            std::process::exit(2);
        }
    }
    let temp = demo.then(|| tempfile::tempdir().expect("demo workspace"));
    let root = temp
        .as_ref()
        .map(|d| d.path().to_owned())
        .or(path)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    if !demo {
        zest_core::load_env();
        if let Err(e) = zest_core::ensure_user_config() {
            eprintln!("Could not initialize provider configuration: {e}");
        }
    }
    gpui_platform::application().run(move |cx| {
        composer::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-n", NewChat, Some("Workspace")),
            KeyBinding::new("ctrl-k", FindChat, Some("Workspace")),
            KeyBinding::new("ctrl-o", OpenFolder, Some("Workspace")),
            KeyBinding::new("ctrl-\\", ToggleSplit, Some("Workspace")),
            KeyBinding::new("escape", ClosePanel, Some("Workspace")),
        ]);
        if let Err(e) = open_window(root, demo, test, cx) {
            eprintln!("Could not open project: {e}");
            std::process::exit(1);
        }
        if !test {
            cx.activate(true);
        }
    });
    drop(temp);
    true
}
