use super::*;
pub(super) mod selftest;
use native_view::button;
use zest_coordinator::{
    ArtifactPage, CreateDelegationJobRequest, DelegationCoordinator, DelegationJobView,
    DelegationStatus as Status, DelegationTargetOption, ReviewerTargetView,
};

#[derive(Clone, Copy)]
pub(super) enum Action {
    Approve,
    Apply,
    Retry,
    Cancel,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Approve => "Approve and run",
            Self::Apply => "Apply reviewed diff",
            Self::Retry => "Prepare retry",
            Self::Cancel => "Cancel delegation",
        }
    }
}

fn actions(status: Status) -> &'static [Action] {
    use self::Action::*;
    match status {
        Status::Planned | Status::AwaitingApproval => &[Approve, Cancel],
        Status::ReadyToApply => &[Apply, Cancel],
        Status::Blocked | Status::ChangesRequested | Status::ApplyConflict => &[Retry, Cancel],
        Status::Failed => &[Retry],
        Status::Queued | Status::WorkerRunning | Status::ReviewRunning => &[Cancel],
        Status::Accepted | Status::Cancelled => &[],
    }
}

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Planned => "Planned",
        Status::AwaitingApproval => "Awaiting approval",
        Status::Queued => "Queued",
        Status::WorkerRunning => "Worker running",
        Status::ReviewRunning => "Reviewer running",
        Status::ReadyToApply => "Ready to apply",
        Status::Accepted => "Applied",
        Status::ChangesRequested => "Changes requested",
        Status::Blocked => "Blocked",
        Status::Failed => "Failed",
        Status::Cancelled => "Cancelled",
        Status::ApplyConflict => "Apply conflict",
    }
}

struct Draft {
    title: Entity<Composer>,
    objective: Entity<Composer>,
    scope: Entity<Composer>,
    checks: Entity<Composer>,
    parent: String,
    worker: Option<usize>,
    reviewer: Option<usize>,
}
impl Draft {
    fn new(parent: String, cx: &mut Context<Workspace>) -> Self {
        let mut field = |placeholder| {
            cx.new(|cx| {
                let mut c = Composer::new(cx);
                c.placeholder = placeholder;
                c.multiline_field = true;
                c
            })
        };
        Self {
            title: field("Card title"),
            objective: field("What should the worker deliver?"),
            scope: field("Allowed project paths, one per line"),
            checks: field("Acceptance commands, one per line (optional)"),
            parent,
            worker: None,
            reviewer: None,
        }
    }
}

pub(super) struct Board {
    pub(super) jobs: Vec<DelegationJobView>,
    pub(super) selected: Option<String>,
    pub(super) artifact: Option<ArtifactPage>,
    pub(super) busy: bool,
    targets: Vec<DelegationTargetOption>,
    draft: Option<Draft>,
    readonly: bool,
    message: String,
    reading: bool,
    next_read: Option<bool>,
    artifact_epoch: u64,
}

impl Board {
    pub(super) fn new(project: &Project, cx: &mut Context<Workspace>) -> Self {
        let mut events = project.board_events.clone();
        cx.spawn(async move |this, cx| {
            while events.changed().await.is_ok() {
                if this
                    .update(cx, |ws, cx| {
                        if ws.panel == Panel::Delegation {
                            ws.load_board(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.on_app_quit(|ws, _| {
            let project = ws.project.clone();
            async move {
                if project.delegations.holds_lock(&project.path) {
                    let _ = native::executor()
                        .spawn(async move { project.delegations.shutdown(&project.path).await })
                        .await;
                }
            }
        })
        .detach();
        Self {
            jobs: vec![],
            selected: None,
            artifact: None,
            busy: false,
            targets: vec![],
            draft: None,
            readonly: false,
            message: String::new(),
            reading: false,
            next_read: None,
            artifact_epoch: 0,
        }
    }
    fn selected_job(&self) -> Option<&DelegationJobView> {
        self.jobs
            .iter()
            .find(|j| Some(&j.job_id) == self.selected.as_ref())
    }
    fn clear_artifact(&mut self) {
        self.artifact_epoch += 1;
        self.artifact = None;
    }
}

impl Workspace {
    pub(super) fn load_board(&mut self, reconcile: bool, cx: &mut Context<Self>) {
        if self.board.reading {
            self.board.next_read = Some(self.board.next_read.unwrap_or(false) || reconcile);
            return;
        }
        self.board.reading = true;
        let project = self.project.clone();
        if reconcile {
            self.board.message.clear();
        }
        let task = native::executor().spawn_blocking(move || {
            let mut locked = None;
            let mut targets = None;
            if reconcile {
                locked = Some(match project.delegations.reconcile(&project.path) {
                    Ok(_) => false,
                    Err(e) if zest_coordinator::is_lock_held_error(&e) => true,
                    Err(e) => return Err(e),
                });
                if !project.demo {
                    targets = Some(DelegationCoordinator::list_targets(&project.path));
                }
            }
            Ok((
                zest_coordinator::list_views(&project.path)?,
                locked,
                targets,
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |ws, cx| {
                let board = &mut ws.board;
                board.reading = false;
                match result {
                    Ok((jobs, locked, targets)) => {
                        if let Some(locked) = locked {
                            board.readonly = locked;
                        }
                        if let Some(targets) = targets {
                            match targets {
                                Ok(targets) if board.draft.is_none() => board.targets = targets,
                                Ok(_) => {}
                                Err(e) => board.message = e,
                            }
                        }
                        let revision = board.selected_job().map(|j| j.updated_at);
                        board.jobs = jobs;
                        if board.selected_job().map(|j| j.updated_at) != revision {
                            board.clear_artifact();
                        }
                    }
                    Err(e) => board.message = e,
                }
                if let Some(reconcile) = board.next_read.take() { ws.load_board(reconcile, cx); }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn select_card(&mut self, id: String, cx: &mut Context<Self>) {
        self.board.selected = Some(id);
        self.board.clear_artifact();
        self.board.draft = None;
        self.board.message.clear();
        cx.notify();
    }

    pub(super) fn show_artifact(
        &mut self,
        name: &'static str,
        offset: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.board.selected.clone() else {
            return;
        };
        self.board.clear_artifact();
        let epoch = self.board.artifact_epoch;
        let project = self.project.clone();
        let task = native::executor().spawn_blocking(move || {
            project
                .delegations
                .artifact_page(&project.path, &id, name, offset)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |ws, cx| {
                if epoch != ws.board.artifact_epoch {
                    return;
                }
                match result {
                    Ok(page) => {
                        ws.board.artifact = Some(page);
                        ws.board.message.clear();
                    }
                    Err(e) => ws.board.message = e,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn board_operation(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(Project) -> Result<DelegationJobView, String> + Send + 'static,
    ) {
        if self.board.busy || self.board.readonly {
            return;
        }
        self.board.busy = true;
        self.board.message.clear();
        let project = self.project.clone();
        let task = native::executor().spawn_blocking(move || op(project));
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |ws, cx| {
                ws.board.busy = false;
                match result {
                    Ok(job) => {
                        ws.board.message = format!("{} · {}", job.title, status_text(job.status));
                        ws.board.selected = Some(job.job_id);
                        ws.board.draft = None;
                        ws.board.clear_artifact();
                    }
                    Err(e) => ws.board.message = e,
                }
                ws.load_board(false, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn act_on_card(
        &mut self,
        action: Action,
        id: String,
        updated_at: u64,
        cx: &mut Context<Self>,
    ) {
        self.board_operation(cx, move |p| match action {
            Action::Approve => p.delegations.approve(&p.path, &id, Some(updated_at)),
            Action::Apply => p.delegations.apply(&p.path, &id, Some(updated_at)),
            Action::Cancel => p.delegations.cancel(&p.path, &id, Some(updated_at)),
            Action::Retry => p.delegations.retry(&p.path, &id, Some(updated_at)),
        });
    }

    fn save_card(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.board.draft else { return };
        let target = |idx: Option<usize>| {
            idx.and_then(|i| self.board.targets.get(i))
                .filter(|t| t.available)
                .map(|t| t.target.clone())
        };
        let Some(worker) = target(d.worker) else {
            self.board.message = "Choose a worker.".into();
            cx.notify();
            return;
        };
        let lines = |f: &Entity<Composer>| {
            f.read(cx)
                .text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let scope = lines(&d.scope);
        let title = d.title.read(cx).text.trim().to_owned();
        let objective = d.objective.read(cx).text.trim().to_owned();
        if scope.is_empty() || title.is_empty() || objective.is_empty() {
            self.board.message = "Enter a title, objective and scope.".into();
            cx.notify();
            return;
        }
        let req = CreateDelegationJobRequest {
            title,
            objective,
            scope,
            worker,
            reviewer: target(d.reviewer).map(|target| ReviewerTargetView::Target { target }),
            chat_id: Some(d.parent.clone()),
            parent_thread_id: d.parent.clone(),
            lane: "feature".into(),
            context: vec![],
            depends_on: vec![],
            acceptance_checks: lines(&d.checks),
            origin_coordinator: None,
            idempotency_key: None,
        };
        self.board_operation(cx, move |p| p.delegations.create_job(&p.path, req));
    }

    pub(super) fn board_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let b = &self.board;
        let mut body = div().flex().flex_col().gap_2().child(button(
            "refresh-board",
            "Refresh",
            cx.listener(|ws, _, _, cx| ws.load_board(true, cx)),
        ));
        if b.readonly {
            body = body.child("Another coordinator owns this project. Cards are read-only.");
        }
        if b.busy {
            body = body.child("Updating delegation…");
        }
        if !b.message.is_empty() {
            body = body.child(b.message.clone());
        }
        if b.draft.is_none() && !b.readonly && !b.busy && b.targets.iter().any(|t| t.available) {
            body = body.child(button(
                "new-card",
                "New delegation",
                cx.listener(|ws, _, w, cx| {
                    let draft = Draft::new(ws.active.clone(), cx);
                    w.focus(&draft.title.read(cx).focus.clone(), cx);
                    ws.board.draft = Some(draft);
                    ws.board.message.clear();
                    cx.notify();
                }),
            ));
        }
        if let Some(d) = &b.draft {
            body = body
                .child("Title")
                .child(d.title.clone())
                .child("Objective")
                .child(d.objective.clone())
                .child("Scope")
                .child(d.scope.clone())
                .child("Acceptance checks")
                .child(d.checks.clone());
            for (review, label, selected) in
                [(false, "Worker", d.worker), (true, "Reviewer", d.reviewer)]
            {
                let mut options = div().flex().flex_wrap().gap_1();
                if review {
                    options = options.child(button(
                        "same-worker",
                        mark("Same target, fresh reviewer", selected.is_none()),
                        cx.listener(|ws, _, _, cx| {
                            if let Some(d) = ws.board.draft.as_mut() {
                                d.reviewer = None;
                                cx.notify();
                            }
                        }),
                    ));
                }
                for (i, t) in b.targets.iter().enumerate().filter(|(_, t)| t.available) {
                    options = options.child(button(
                        (if review { "reviewer" } else { "worker" }, i),
                        mark(&t.label, selected == Some(i)),
                        cx.listener(move |ws, _, _, cx| {
                            if let Some(d) = ws.board.draft.as_mut() {
                                if review {
                                    d.reviewer = Some(i);
                                } else {
                                    d.worker = Some(i);
                                }
                                cx.notify();
                            }
                        }),
                    ));
                }
                body = body.child(label).child(options);
            }
            if !b.busy {
                body = body
                    .child(button(
                        "create-card",
                        "Create for approval",
                        cx.listener(|ws, _, _, cx| ws.save_card(cx)),
                    ))
                    .child(button(
                        "close-draft",
                        "Close draft",
                        cx.listener(|ws, _, _, cx| {
                            ws.board.draft = None;
                            cx.notify();
                        }),
                    ));
            }
            return body;
        }
        if b.jobs.is_empty() {
            return body.child("No delegations in this project.");
        }
        for (i, j) in b.jobs.iter().enumerate() {
            let id = j.job_id.clone();
            body = body.child(button(
                ("card", i),
                format!("{} · {}", j.title, status_text(j.status)),
                cx.listener(move |ws, _, _, cx| ws.select_card(id.clone(), cx)),
            ));
        }
        let Some(j) = b.selected_job() else {
            return body;
        };
        body = body
            .child(j.objective.clone())
            .child(format!("Scope · {}", j.scope.join(", ")))
            .child(format!(
                "Worker · {}    Reviewer · {}",
                j.agent, j.reviewer_agent
            ));
        for line in j.context.iter().chain(&j.depends_on) {
            body = body.child(line.clone());
        }
        for a in &j.attempts {
            body = body.child(format!("{} · {}", a.role, a.agent));
        }
        if let Some(summary) = &j.worker_summary {
            body = body.child(summary.clone());
        }
        if let Some(error) = &j.error {
            body = body.child(error.clone());
        }
        for check in &j.acceptance_checks {
            body = body.child(format!(
                "{:?} · {}\n{}",
                check.status, check.command, check.output
            ));
        }
        for finding in &j.reviewer_findings {
            body = body.child(format!(
                "{:?} · {} · {}",
                finding.severity, finding.path, finding.message
            ));
        }
        if !j.changed_files.is_empty() {
            body = body.child(format!("Changed files\n{}", j.changed_files.join("\n")));
        }
        let mut controls = div().flex().flex_wrap().gap_2();
        if !b.readonly && !b.busy {
            for &action in actions(j.status) {
                let id = j.job_id.clone();
                let rev = j.updated_at;
                controls = controls.child(button(
                    action.label(),
                    action.label(),
                    cx.listener(move |ws, _, _, cx| ws.act_on_card(action, id.clone(), rev, cx)),
                ));
            }
        }
        body = body.child(controls);
        let mut artifacts = div().flex().flex_wrap().gap_2();
        for &name in zest_coordinator::ALLOWED_ARTIFACTS {
            artifacts = artifacts.child(button(
                name,
                name,
                cx.listener(move |ws, _, _, cx| ws.show_artifact(name, 0, cx)),
            ));
        }
        body = body.child(artifacts);
        if let Some(page) = &b.artifact {
            let Some(&name) = zest_coordinator::ALLOWED_ARTIFACTS
                .iter()
                .find(|&&n| n == page.name)
            else {
                return body;
            };
            if page.offset > 0 {
                let prev = page
                    .offset
                    .saturating_sub(zest_coordinator::ARTIFACT_PAGE_BYTES as u64);
                body = body.child(button(
                    "prev-artifact",
                    "Previous page",
                    cx.listener(move |ws, _, _, cx| ws.show_artifact(name, prev, cx)),
                ));
            }
            if let Some(next) = page.next_offset {
                body = body.child(button(
                    "next-artifact",
                    "Next page",
                    cx.listener(move |ws, _, _, cx| ws.show_artifact(name, next, cx)),
                ));
            }
            body = body.child(
                div()
                    .font_family(MONO)
                    .text_size(px(13.))
                    .child(page.content.clone()),
            );
        }
        body
    }
}
