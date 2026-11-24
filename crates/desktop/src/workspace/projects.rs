use super::*;
use std::collections::HashSet;
use std::path::Path;
use worktree_registry::WorktreeChatRecord;
use zest_core::{
    PullRequestLink, ThreadGitContext, ThreadStore, pull_request, worktree_chat, worktree_registry,
};

#[derive(Clone)]
struct LinkedChat {
    project: PathBuf,
    thread: String,
    title: String,
    context: ThreadGitContext,
    pr: PullRequestLink,
}

struct Review {
    chat: LinkedChat,
    checks: Option<Result<pull_request::PullRequestStatus, String>>,
    diff: Option<Result<zest_core::WorkspaceChangeSet, String>>,
    page: usize,
}

#[derive(Default)]
pub(super) struct ProjectPanels {
    worktrees: Vec<(
        WorktreeChatRecord,
        Option<worktree_chat::ChatWorktreeStatus>,
    )>,
    creating: bool,
    linked: Vec<LinkedChat>,
    review: Option<Review>,
    status_filter: usize,
    pub(super) refresh_task: Option<Task<()>>,
}

fn list_links(current: &Path, registry: &Path) -> Vec<LinkedChat> {
    let known = std::fs::read(registry.join("known-workspaces.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Vec<PathBuf>>(&raw).ok())
        .unwrap_or_default();
    let records = worktree_registry::load_worktree_chat_records_from(registry);
    let hidden: HashSet<_> = records
        .iter()
        .filter(|r| !r.visible)
        .map(|r| r.path.clone())
        .collect();
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for root in std::iter::once(current.to_owned())
        .chain(known)
        .chain(records.iter().filter(|r| r.visible).map(|r| r.path.clone()))
    {
        let Ok(path) = root.canonicalize() else {
            continue;
        };
        if !seen.insert(path.clone())
            || hidden.contains(&path)
            || !path.join(".zest/threads").is_dir()
        {
            continue;
        }
        let Ok(rows) = ThreadStore::open(&path).and_then(|s| s.list()) else {
            continue;
        };
        for row in rows {
            let title = chat_title(&row);
            let Some(context) = row.git_context else {
                continue;
            };
            let Some(pr) = context
                .pull_request
                .clone()
                .filter(|p| zest_core::workspace_changes::is_safe_pr_number(p.number))
            else {
                continue;
            };
            links.push((
                row.updated_at,
                LinkedChat {
                    project: path.clone(),
                    thread: row.id,
                    title,
                    context,
                    pr,
                },
            ));
        }
    }
    links.sort_by_key(|(updated, _)| std::cmp::Reverse(*updated));
    let mut urls = HashSet::new();
    links
        .into_iter()
        .filter_map(|(_, link)| urls.insert(link.pr.url.clone()).then_some(link))
        .collect()
}

fn status_label(status: Option<worktree_chat::ChatWorktreeStatus>) -> String {
    let Some(s) = status else {
        return String::new();
    };
    if s.ahead == 0 && s.behind == 0 && s.dirty == 0 {
        return String::new();
    }
    format!(
        " · {} ahead, {} behind, {} uncommitted",
        s.ahead, s.behind, s.dirty
    )
}

impl Workspace {
    pub(super) fn load_worktrees(&mut self, cx: &mut Context<Self>) {
        let dir = self.project.registry_dir.clone();
        let epoch = self.report_epoch;
        self.report = "Reading worktrees…".into();
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move {
                    worktree_registry::load_worktree_chat_records_from(&dir)
                        .into_iter()
                        .filter(|r| r.visible)
                        .map(|r| {
                            let status = worktree_chat::chat_worktree_status(
                                &r.path,
                                &r.source_root,
                                r.base_commit.as_deref(),
                            );
                            (r, status)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if ws.panel == Panel::Worktrees && ws.report_epoch == epoch {
                    ws.projects.worktrees = rows;
                    ws.report.clear();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.visit_project(path, None, cx);
    }

    fn visit_project(&mut self, path: PathBuf, linked: Option<LinkedChat>, cx: &mut Context<Self>) {
        let registry = self.project.registry_dir.clone();
        let demo = self.project.demo;
        cx.spawn(async move |this, cx| {
            let result = cx.update(|cx| {
                let project =
                    Project::open_with_registry(path, demo, registry).map_err(|e| e.to_string())?;
                let handle = open_project_window(project, false, cx)?;
                if let Some(chat) = linked {
                    handle
                        .update(cx, |ws, w, cx| {
                            ws.open(&chat.thread, w, cx);
                            if ws.active == chat.thread {
                                ws.begin_review(chat, cx);
                            }
                        })
                        .map_err(|e| e.to_string())?;
                }
                Ok::<_, String>(())
            });
            if let Err(e) = result {
                let _ = this.update(cx, |ws, cx| {
                    ws.toast(ToastKind::Error, e, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn create_worktree(&mut self, cx: &mut Context<Self>) {
        if self.projects.creating {
            return;
        }
        self.projects.creating = true;
        let path = self.project.path.clone();
        let dir = self.project.registry_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { worktree_registry::create(&dir, &path) })
                .await;
            let _ = this.update(cx, |ws, cx| {
                ws.projects.creating = false;
                match result {
                    Ok(record) => {
                        ws.open_worktree(record.path, cx);
                        if ws.panel == Panel::Worktrees {
                            ws.load_worktrees(cx);
                        }
                    }
                    Err(e) => ws.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn worktrees_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(button(
                "new-worktree",
                if self.projects.creating {
                    "Creating checkout…"
                } else {
                    "New chat in worktree"
                },
                cx.listener(|ws, _, _, cx| ws.create_worktree(cx)),
            ))
            .child(button(
                "refresh-worktrees",
                "Refresh",
                cx.listener(|ws, _, w, cx| ws.page(Panel::Worktrees, w, cx)),
            ));
        if self.project.path != self.project.config_root {
            body = body.child(button(
                "source-project",
                "Open source project",
                cx.listener(|ws, _, _, cx| ws.open_worktree(ws.project.config_root.clone(), cx)),
            ));
        }
        if !self.report.is_empty() {
            body = body.child(self.report.clone());
        } else if self.projects.worktrees.is_empty() {
            body = body.child(crate::ui::hint("No saved worktree chats."));
        }
        for (i, (record, status)) in self.projects.worktrees.iter().enumerate() {
            let path = record.path.clone();
            let hide = path.clone();
            body = body.child(
                div()
                    .flex()
                    .gap_1()
                    .child(button(
                        ("worktree", i),
                        format!("{}{}", record.branch, status_label(*status)),
                        cx.listener(move |ws, _, _, cx| ws.open_worktree(path.clone(), cx)),
                    ))
                    .child(button(
                        ("hide-worktree", i),
                        "Hide from list",
                        cx.listener(move |ws, _, w, cx| {
                            match worktree_registry::set_visibility(
                                &ws.project.registry_dir,
                                &hide,
                                false,
                            ) {
                                Ok(()) => ws.page(Panel::Worktrees, w, cx),
                                Err(e) => ws.toast(ToastKind::Error, e, cx),
                            }
                        }),
                    )),
            );
        }
        body
    }

    pub(super) fn load_pull_requests(&mut self, cx: &mut Context<Self>) {
        self.projects.review = None;
        self.projects.linked.clear();
        self.report = "Looking for linked pull requests…".into();
        let chat = self.chat(cx);
        let path = self.project.path.clone();
        let registry = self.project.registry_dir.clone();
        let epoch = self.report_epoch;
        let lookup = native::executor().spawn(async move {
            let context = chat.thread.lock().ok().and_then(|t| t.git_context.clone());
            if let Some((_, Some(branch))) = worktree_chat::chat_git_baseline(&path)
                && let Some(context) = context
                && context.branch.as_deref() == Some(&branch)
                && let pull_request::PullRequestLookup::Found(pr) =
                    pull_request::lookup_pull_request_cached(&path, Some(&branch)).await
                && context.pull_request.as_ref() != Some(&pr)
            {
                chat.edit(|t| {
                    t.git_context
                        .get_or_insert_with(Default::default)
                        .pull_request = Some(pr);
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
            }
            tokio::task::spawn_blocking(move || list_links(&path, &registry))
                .await
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = lookup.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |ws, cx| {
                if ws.report_epoch == epoch && ws.panel == Panel::PullRequests {
                    match result {
                        Ok(rows) => {
                            ws.projects.linked = rows;
                            ws.report.clear();
                        }
                        Err(e) => ws.report = e,
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_linked_chat(&mut self, chat: LinkedChat, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.path == chat.project {
            self.open(&chat.thread, window, cx);
            if self.active == chat.thread {
                self.begin_review(chat, cx);
            }
            return;
        }
        self.visit_project(chat.project.clone(), Some(chat), cx);
    }

    fn begin_review(&mut self, chat: LinkedChat, cx: &mut Context<Self>) {
        self.panel = Panel::PullRequests;
        self.report_epoch += 1;
        self.report.clear();
        let epoch = self.report_epoch;
        let root = chat.project.clone();
        let number = chat.pr.number;
        let context = chat.context.clone();
        self.projects.review = Some(Review {
            chat,
            checks: None,
            diff: None,
            page: 0,
        });
        let diff = native::executor()
            .spawn(async move { pull_request::diff(&root, number, Some(&context)).await });
        cx.spawn(async move |this, cx| {
            let result = diff.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |ws, cx| {
                if ws.panel == Panel::PullRequests
                    && ws.report_epoch == epoch
                    && let Some(review) = &mut ws.projects.review
                {
                    review.diff = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
        self.refresh_checks(cx);
    }

    fn refresh_checks(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.projects.review else {
            return;
        };
        let path = review.chat.project.clone();
        let number = review.chat.pr.number;
        let epoch = self.report_epoch;
        self.projects.refresh_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let root = path.clone();
                let result = native::executor()
                    .spawn(async move { pull_request::status(&root, number).await })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                let keep = this
                    .update(cx, |ws, cx| {
                        if ws.panel != Panel::PullRequests || ws.report_epoch != epoch {
                            return false;
                        }
                        let Some(review) = &mut ws.projects.review else {
                            return false;
                        };
                        review.checks = Some(result);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
            }
        }));
        cx.notify();
    }

    pub(super) fn pull_requests_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::{self, Icon, alpha};
        use crate::ui::{self, Variant};

        let mut body = div().flex().flex_col().gap_4();
        if let Some(review) = &self.projects.review {
            let url = review.chat.pr.url.clone();
            let header = ui::card()
                .rounded(px(12.))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(ui::button(
                            "back-prs",
                            Variant::Ghost,
                            Some(Icon::ArrowLeft),
                            "All pull requests",
                            cx.listener(|ws, _, w, cx| ws.page(Panel::PullRequests, w, cx)),
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(ui::button(
                                    "refresh-checks",
                                    Variant::Outline,
                                    Some(if review.checks.is_none() {
                                        Icon::LoaderCircle
                                    } else {
                                        Icon::RefreshCw
                                    }),
                                    "Refresh checks",
                                    cx.listener(|ws, _, _, cx| ws.refresh_checks(cx)),
                                ))
                                .child(ui::button(
                                    "open-pr",
                                    Variant::Outline,
                                    Some(Icon::ExternalLink),
                                    "Open on GitHub",
                                    cx.listener(move |ws, _, _, cx| ws.open_web_link(&url, cx)),
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_3()
                        .child(
                            theme::icon(Icon::GitPullRequest, 18.)
                                .mt(px(2.))
                                .text_color(rgb(theme::primary())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme::foreground()))
                                        .child(format!(
                                            "#{} {}",
                                            review.chat.pr.number, review.chat.pr.title
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(theme::muted_foreground()))
                                        .child(format!(
                                            "{} · {}",
                                            review.chat.title, review.chat.pr.state
                                        )),
                                ),
                        ),
                );
            body = body.child(header);

            let mut checks_card = ui::card()
                .rounded(px(10.))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Checks"),
                );
            match &review.checks {
                None => {
                    checks_card = checks_card
                        .child(ui::loading_row("pr-checks-loading", "Fetching checks…"));
                }
                Some(Err(e)) => {
                    checks_card = checks_card.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(theme::destructive()))
                            .child(e.clone()),
                    );
                }
                Some(Ok(status)) => {
                    checks_card = checks_card.child(ui::pill(
                        None,
                        format!("{} · {} checks", status.state, status.checks.len()),
                    ));
                    for (i, check) in status.checks.iter().enumerate() {
                        let row_line = div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py(px(6.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(alpha(theme::border(), 0.6))
                            .text_size(px(12.))
                            .child(div().flex_1().min_w_0().truncate().child(check.name.clone()))
                            .child(ui::pill(None, check.status.clone()));
                        if let Some(url) = check.url.clone() {
                            checks_card = checks_card.child(
                                row_line
                                    .id(("check-log", i))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(theme::secondary())))
                                    .child(theme::icon(Icon::ExternalLink, 12.))
                                    .on_click(cx.listener(move |ws, _, _, cx| {
                                        ws.open_web_link(&url, cx)
                                    })),
                            );
                        } else {
                            checks_card = checks_card.child(row_line);
                        }
                    }
                }
            }
            body = body.child(checks_card);

            let mut diff_card = ui::card()
                .rounded(px(10.))
                .p_3()
                .flex()
                .flex_col()
                .gap_3();
            match &review.diff {
                None => {
                    diff_card = diff_card
                        .child(ui::loading_row("pr-diff-loading", "Fetching pull request diff…"));
                }
                Some(Err(e)) => {
                    diff_card = diff_card.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(theme::destructive()))
                            .child(e.clone()),
                    );
                }
                Some(Ok(snapshot)) if snapshot.unavailable => {
                    diff_card = diff_card.child(ui::hint("Pull request diff unavailable."));
                }
                Some(Ok(snapshot)) => {
                    let start = review.page * 80;
                    let has_next = snapshot.diff.lines().nth(start + 80).is_some();
                    diff_card = diff_card
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!(
                                            "{} files · +{} −{}",
                                            snapshot.changed_files.len(),
                                            snapshot.additions,
                                            snapshot.deletions
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .when(start > 0, |d| {
                                            d.child(ui::button(
                                                "previous-pr-diff",
                                                Variant::Outline,
                                                None,
                                                "Previous 80 lines",
                                                cx.listener(|ws, _, _, cx| {
                                                    if let Some(r) = &mut ws.projects.review {
                                                        r.page = r.page.saturating_sub(1);
                                                        cx.notify();
                                                    }
                                                }),
                                            ))
                                        })
                                        .when(has_next, |d| {
                                            d.child(ui::button(
                                                "next-pr-diff",
                                                Variant::Outline,
                                                None,
                                                "Next 80 lines",
                                                cx.listener(|ws, _, _, cx| {
                                                    if let Some(r) = &mut ws.projects.review {
                                                        r.page += 1;
                                                        cx.notify();
                                                    }
                                                }),
                                            ))
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(alpha(theme::border(), 0.7))
                                .bg(rgb(theme::input_bg()))
                                .p_3()
                                .font_family(MONO)
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(rgb(theme::secondary_foreground()))
                                .child(
                                    snapshot
                                        .diff
                                        .lines()
                                        .skip(start)
                                        .take(80)
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                ),
                        )
                        .when(snapshot.truncated, |d| {
                            d.child(ui::hint("Patch truncated at the display limit."))
                        });
                }
            }
            return body.child(diff_card);
        }

        let filters = ["All", "Open", "Draft", "Merged", "Closed"];
        let query = self.filter.read(cx).text.trim().to_lowercase();
        let visible: Vec<_> = self
            .projects
            .linked
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let status = &row.pr.state;
                let matches_status = match self.projects.status_filter {
                    0 => true,
                    2 => status.eq_ignore_ascii_case("open") && row.pr.is_draft,
                    n => status.eq_ignore_ascii_case(filters[n]),
                };
                if !matches_status {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                let haystack = format!(
                    "{} {} {} {}",
                    zest_core::display_path(&row.project),
                    row.pr.title,
                    row.pr.number,
                    row.context.branch.as_deref().unwrap_or("")
                )
                .to_lowercase();
                haystack.contains(&query)
            })
            .collect();

        let search_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(10.))
                    .py(px(2.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .bg(rgb(theme::background()))
                    .child(theme::icon(Icon::Search, 15.).text_color(rgb(theme::muted_foreground())))
                    .child(div().flex_1().min_w_0().child(self.filter.clone())),
            )
            .child(ui::button(
                "refresh-prs",
                Variant::Outline,
                Some(Icon::RefreshCw),
                "Refresh",
                cx.listener(|ws, _, w, cx| ws.page(Panel::PullRequests, w, cx)),
            ));

        let count_label = format!(
            "{} {}",
            visible.len(),
            if visible.len() == 1 {
                "pull request"
            } else {
                "pull requests"
            }
        );
        let mut filter_bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .pb_3()
            .border_b_1()
            .border_color(alpha(theme::border(), 0.6));
        for (index, name) in filters.iter().enumerate() {
            let active = self.projects.status_filter == index;
            filter_bar = filter_bar.child(
                div()
                    .id(("pr-filter", index))
                    .role(Role::Button)
                    .aria_label(*name)
                    .tab_index(0)
                    .cursor_pointer()
                    .px_3()
                    .py(px(6.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(if active {
                        theme::foreground()
                    } else {
                        theme::muted_foreground()
                    }))
                    .when(active, |d| d.bg(rgb(theme::secondary())))
                    .when(!active, |d| {
                        d.hover(|s| {
                            s.bg(alpha(theme::secondary(), 0.55))
                                .text_color(rgb(theme::foreground()))
                        })
                    })
                    .child(*name)
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.projects.status_filter = index;
                        cx.notify();
                    })),
            );
        }
        filter_bar = filter_bar.child(div().flex_1()).child(
            div()
                .px_2()
                .text_size(px(12.))
                .text_color(rgb(theme::muted_foreground()))
                .child(count_label),
        );

        body = body.child(div().flex().flex_col().gap_3().child(search_row).child(filter_bar));

        if !self.report.is_empty() {
            body = body.child(
                div()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(alpha(theme::destructive(), 0.4))
                    .bg(alpha(theme::destructive(), 0.1))
                    .px_3()
                    .py_2()
                    .text_size(px(12.5))
                    .text_color(rgb(theme::destructive()))
                    .child(self.report.clone()),
            );
        }

        if visible.is_empty() {
            let has_any = !self.projects.linked.is_empty();
            body = body.child(ui::empty_state(
                Icon::GitPullRequest,
                if has_any {
                    "No matching pull requests"
                } else {
                    "No linked pull requests yet"
                },
                if has_any {
                    "Try another search or status."
                } else {
                    "Open a project chat on a branch with a pull request. It will appear here once Zest detects it."
                },
            ));
        } else {
            let mut list_card = ui::card()
                .rounded(px(12.))
                .overflow_hidden()
                .flex()
                .flex_col();
            for (row_idx, (i, row)) in visible.into_iter().enumerate() {
                let chat = row.clone();
                let url = row.pr.url.clone();
                let project_label = row
                    .project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| zest_core::display_path(&row.project));
                let state_text = if row.pr.is_draft && row.pr.state.eq_ignore_ascii_case("open") {
                    "draft".to_string()
                } else {
                    row.pr.state.to_lowercase()
                };
                let mut meta_parts = vec![
                    project_label,
                    format!("#{}", row.pr.number),
                    state_text,
                ];
                if let Some(branch) = row.context.branch.as_deref().filter(|b| !b.is_empty()) {
                    meta_parts.push(branch.to_string());
                }
                let title = if row.pr.title.trim().is_empty() {
                    format!("Pull request #{}", row.pr.number)
                } else {
                    row.pr.title.clone()
                };
                list_card = list_card.child(
                    div()
                        .px_4()
                        .py_3()
                        .when(row_idx > 0, |d| {
                            d.border_t_1().border_color(alpha(theme::border(), 0.7))
                        })
                        .hover(|s| s.bg(alpha(theme::secondary(), 0.45)))
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(
                            theme::icon(Icon::GitPullRequest, 18.)
                                .text_color(rgb(theme::primary())),
                        )
                        .child(
                            div()
                                .id(("pr", i))
                                .role(Role::Button)
                                .aria_label(SharedString::from(format!(
                                    "#{} {}",
                                    row.pr.number, title
                                )))
                                .tab_index(0)
                                .cursor_pointer()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(theme::foreground()))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(rgb(theme::muted_foreground()))
                                        .child(meta_parts.join(" · ")),
                                )
                                .on_click(cx.listener(move |ws, _, w, cx| {
                                    ws.open_linked_chat(chat.clone(), w, cx)
                                })),
                        )
                        .child(ui::icon_button(
                            ("pr-github", i),
                            Icon::ExternalLink,
                            format!("Open pull request #{} on GitHub", row.pr.number),
                            cx.listener(move |ws, _, _, cx| ws.open_web_link(&url, cx)),
                        )),
                );
            }
            body = body.child(list_card);
        }

        body
    }

    fn open_web_link(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Err(e) = zest_core::codex_oauth::open_http_url(url) {
            self.toast(ToastKind::Error, e, cx);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{status_label, worktree_chat};
    #[test]
    fn worktree_counts_are_quiet_for_clean_and_unavailable_status() {
        assert_eq!(status_label(None), "");
        assert_eq!(status_label(Some(Default::default())), "");
        assert_eq!(
            status_label(Some(worktree_chat::ChatWorktreeStatus {
                ahead: 1,
                behind: 2,
                dirty: 3
            })),
            " · 1 ahead, 2 behind, 3 uncommitted"
        );
    }
}
