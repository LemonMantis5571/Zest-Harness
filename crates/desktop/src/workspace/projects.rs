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
        let mut body = div().flex().flex_col().gap_2();
        if let Some(review) = &self.projects.review {
            body = body
                .child(button(
                    "back-prs",
                    "All pull requests",
                    cx.listener(|ws, _, w, cx| ws.page(Panel::PullRequests, w, cx)),
                ))
                .child(format!(
                    "#{} {} · {}",
                    review.chat.pr.number, review.chat.pr.title, review.chat.title
                ));
            let url = review.chat.pr.url.clone();
            body = body
                .child(button(
                    "open-pr",
                    "Open on GitHub",
                    cx.listener(move |ws, _, _, cx| ws.open_web_link(&url, cx)),
                ))
                .child(button(
                    "refresh-checks",
                    "Refresh checks",
                    cx.listener(|ws, _, _, cx| ws.refresh_checks(cx)),
                ));
            match &review.checks {
                None => body = body.child(crate::ui::hint("Fetching checks…")),
                Some(Err(e)) => body = body.child(e.clone()),
                Some(Ok(status)) => {
                    body = body.child(format!("{} · {} checks", status.state, status.checks.len()));
                    for (i, check) in status.checks.iter().enumerate() {
                        let label = format!("{} · {}", check.name, check.status);
                        if let Some(url) = check.url.clone() {
                            body = body.child(button(
                                ("check-log", i),
                                label,
                                cx.listener(move |ws, _, _, cx| ws.open_web_link(&url, cx)),
                            ));
                        } else {
                            body = body.child(label);
                        }
                    }
                }
            }
            match &review.diff {
                None => body = body.child(crate::ui::hint("Fetching pull request diff…")),
                Some(Err(e)) => body = body.child(e.clone()),
                Some(Ok(snapshot)) if snapshot.unavailable => {
                    body = body.child(crate::ui::hint("Pull request diff unavailable."))
                }
                Some(Ok(snapshot)) => {
                    body = body.child(format!(
                        "{} files · +{} −{}",
                        snapshot.changed_files.len(),
                        snapshot.additions,
                        snapshot.deletions
                    ));
                    let start = review.page * 80;
                    body = body.child(
                        div().font_family(MONO).child(
                            snapshot
                                .diff
                                .lines()
                                .skip(start)
                                .take(80)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ),
                    );
                    if start > 0 {
                        body = body.child(button(
                            "previous-pr-diff",
                            "Previous 80 lines",
                            cx.listener(|ws, _, _, cx| {
                                if let Some(r) = &mut ws.projects.review {
                                    r.page = r.page.saturating_sub(1);
                                    cx.notify();
                                }
                            }),
                        ));
                    }
                    if snapshot.diff.lines().nth(start + 80).is_some() {
                        body = body.child(button(
                            "next-pr-diff",
                            "Next 80 lines",
                            cx.listener(|ws, _, _, cx| {
                                if let Some(r) = &mut ws.projects.review {
                                    r.page += 1;
                                    cx.notify();
                                }
                            }),
                        ));
                    }
                    if snapshot.truncated {
                        body = body.child(crate::ui::hint("Patch truncated at the display limit."));
                    }
                }
            }
            return body;
        }
        body = body.child(self.filter.clone()).child(button(
            "refresh-prs",
            "Refresh",
            cx.listener(|ws, _, w, cx| ws.page(Panel::PullRequests, w, cx)),
        ));
        let filters = ["All", "Open", "Draft", "Merged", "Closed"];
        let mut buttons = div().flex().gap_1();
        for (index, name) in filters.iter().enumerate() {
            buttons = buttons.child(button(
                ("pr-filter", index),
                mark(name, self.projects.status_filter == index),
                cx.listener(move |ws, _, _, cx| {
                    ws.projects.status_filter = index;
                    cx.notify();
                }),
            ));
        }
        body = body.child(buttons);
        let query = self.filter.read(cx).text.to_lowercase();
        let mut count = 0;
        for (i, row) in self.projects.linked.iter().enumerate() {
            let status = &row.pr.state;
            let label = format!(
                "{} · #{} {} · {} · {}",
                zest_core::display_path(&row.project),
                row.pr.number,
                row.pr.title,
                row.context.branch.as_deref().unwrap_or(""),
                status
            );
            let matches = match self.projects.status_filter {
                0 => true,
                2 => status == "OPEN" && row.pr.is_draft,
                n => status.eq_ignore_ascii_case(filters[n]),
            };
            if matches && label.to_lowercase().contains(&query) {
                let chat = row.clone();
                body = body.child(button(
                    ("pr", i),
                    label,
                    cx.listener(move |ws, _, w, cx| ws.open_linked_chat(chat.clone(), w, cx)),
                ));
                count += 1;
            }
        }
        if !self.report.is_empty() {
            body = body.child(self.report.clone());
        } else if count == 0 {
            body=body.child("No matching pull requests. Open this panel from a Git chat with a pull request on its branch.");
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
