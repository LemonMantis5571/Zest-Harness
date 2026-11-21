//! The workspace frame drawn like the React UI: sidebar, chat header and page frame.

use super::*;
use crate::theme::{self, Icon, alpha, icon};
use crate::ui;

/// Settings-type panels, shown as tabs under Customize (the React UI's CustomizePanel).
pub(super) const CUSTOMIZE: &[(Panel, &str, Icon)] = &[
    (Panel::Settings, "Models", Icon::Layers),
    (Panel::Commands, "Skills & MCP", Icon::Plug),
    (Panel::Extras, "Add-ons", Icon::Puzzle),
    (Panel::Usage, "Usage", Icon::Gauge),
];

/// Project tools, shown as tabs under Workbench (the React UI's WorkbenchPanel).
pub(super) const WORKBENCH: &[(Panel, &str, Icon)] = &[
    (Panel::Changes, "Changes", Icon::FileDiff),
    (Panel::Files, "Files", Icon::Files),
    (Panel::Jobs, "Jobs", Icon::SquareTerminal),
    (Panel::Checkpoints, "Checkpoints", Icon::History),
    (Panel::Delegation, "Delegation", Icon::Workflow),
    (Panel::Worktrees, "Worktrees", Icon::FolderGit2),
];

const SIDEBAR_LINE: u32 = 0x202225;
const SECTION_LINE: u32 = 0x1c1d20;
const SELECTED_ROW: u32 = 0x202336;

/// Relative age the way the React sidebar prints it: "now", "45s", "6m", "8h", "2d".
pub(super) fn age(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..5 => "now".into(),
        5..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One chat in the sidebar list.
pub(super) fn chat_row(
    id: SharedString,
    glyph: Icon,
    title: String,
    age: String,
    selected: bool,
    busy: bool,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .h(px(30.))
        .py(px(1.))
        .child(
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(SharedString::from(title.clone()))
                .tab_index(0)
                .cursor_pointer()
                .relative()
                .h(px(28.))
                .pl(px(10.))
                .pr_1()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(6.))
                .text_size(px(13.))
                .text_color(rgb(theme::FOREGROUND))
                .when(selected, |d| {
                    d.bg(rgb(SELECTED_ROW)).child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(4.))
                            .bottom(px(4.))
                            .w(px(2.))
                            .rounded_full()
                            .bg(rgb(theme::PRIMARY)),
                    )
                })
                .when(!selected, |d| d.hover(|s| s.bg(rgb(theme::SECONDARY))))
                .focus_visible(|s| s.bg(rgb(theme::ACCENT)))
                .child(
                    icon(glyph, 16.)
                        .text_color(if busy { rgb(theme::PRIMARY) } else { rgb(theme::MUTED_FOREGROUND) }),
                )
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_none()
                        .pr_1()
                        .text_size(px(10.))
                        .text_color(rgb(theme::MUTED_FOREGROUND))
                        .child(age),
                )
                .on_click(click),
        )
        .into_any_element()
}

fn kbd(key: &'static str) -> Div {
    div()
        .px(px(5.))
        .h(px(18.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .text_size(px(10.))
        .text_color(rgb(theme::MUTED_FOREGROUND))
        .child(key)
}

impl Workspace {
    fn location(&self) -> (Panel, String) {
        (self.panel, self.active.clone())
    }

    /// Records where the user is so the sidebar's back and forward arrows can return there.
    pub(super) fn track_navigation(&mut self) {
        let here = self.location();
        if self.nav.get(self.nav_index) != Some(&here) {
            self.nav.truncate(self.nav_index + 1);
            self.nav.push(here);
            self.nav_index = self.nav.len() - 1;
        }
    }

    fn go(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.nav_index.checked_add_signed(step).filter(|i| *i < self.nav.len()) else {
            return;
        };
        self.nav_index = index;
        let (panel, id) = self.nav[index].clone();
        if id != self.active || panel == Panel::Chat {
            self.open(&id, window, cx);
        }
        if panel != Panel::Chat {
            self.page(panel, window, cx);
        }
    }

    fn page_group(&self) -> Option<&'static [(Panel, &'static str, Icon)]> {
        [CUSTOMIZE, WORKBENCH]
            .into_iter()
            .find(|group| group.iter().any(|(p, _, _)| *p == self.panel))
    }

    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let in_customize = CUSTOMIZE.iter().any(|(p, _, _)| *p == self.panel);
        if !self.sidebar_open {
            return div()
                .w(px(44.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.))
                .py(px(6.))
                .bg(rgb(theme::SIDEBAR))
                .border_r_1()
                .border_color(rgb(SIDEBAR_LINE))
                .child(ui::icon_button("expand-sidebar", Icon::PanelLeft, "Show sidebar", cx.listener(|ws, _, _, cx| {
                    ws.sidebar_open = true;
                    cx.notify();
                })))
                .child(div().h(px(6.)))
                .child(ui::icon_button("new", Icon::SquarePen, "New chat", cx.listener(|ws, _, w, cx| ws.new_chat(w, cx))))
                .child(ui::icon_button("nav-search", Icon::Search, "Search", cx.listener(|ws, _, w, cx| ws.page(Panel::Search, w, cx))))
                .child(ui::icon_button("nav-prs", Icon::GitPullRequest, "Pull requests", cx.listener(|ws, _, w, cx| ws.page(Panel::PullRequests, w, cx))))
                .child(ui::icon_button("nav-customize", Icon::SlidersHorizontal, "Customize", cx.listener(|ws, _, w, cx| ws.page(Panel::Settings, w, cx))))
                .into_any_element();
        }

        let now = now_secs();
        let history = uniform_list(
            "history",
            self.draft_ids.len() + self.history.len(),
            cx.processor(move |ws, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let (id, title, glyph, when, busy) = if let Some(id) = ws.draft_ids.get(index) {
                            let chat = ws.chats.get(id)?.read(cx);
                            let line = first_line(&chat.composer.read(cx).text);
                            let title = if line.is_empty() { "New chat".into() } else { format!("Draft · {line}") };
                            (id, title, Icon::SquarePen, "draft".to_string(), false)
                        } else {
                            let row = ws.history.get(index - ws.draft_ids.len())?;
                            let busy = ws.chats.get(&row.id).is_some_and(|c| c.read(cx).turn_active);
                            let glyph = if busy {
                                Icon::LoaderCircle
                            } else if row.pinned {
                                Icon::Pin
                            } else if row.git_context.as_ref().is_some_and(|g| g.pull_request.is_some()) {
                                Icon::GitPullRequest
                            } else {
                                Icon::MessageSquare
                            };
                            (&row.id, chat_title(row), glyph, age(row.updated_at, now), busy)
                        };
                        let selected = &ws.active == id && ws.panel == Panel::Chat;
                        let target = id.clone();
                        Some(chat_row(
                            SharedString::from(id.clone()),
                            glyph,
                            title,
                            when,
                            selected,
                            busy,
                            cx.listener(move |ws, _, w, cx| ws.open(&target, w, cx)),
                        ))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();

        let project_name = self
            .project
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let top = div()
            .h(px(40.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(rgb(SIDEBAR_LINE))
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(
                        ui::icon_button("nav-back", Icon::ArrowLeft, "Back", cx.listener(|ws, _, w, cx| ws.go(-1, w, cx)))
                            .when(self.nav_index == 0, |d| d.opacity(0.5)),
                    )
                    .child(
                        ui::icon_button("nav-forward", Icon::ArrowRight, "Forward", cx.listener(|ws, _, w, cx| ws.go(1, w, cx)))
                            .when(self.nav_index + 1 >= self.nav.len(), |d| d.opacity(0.5)),
                    ),
            )
            .child(ui::icon_button("collapse-sidebar", Icon::PanelLeft, "Hide sidebar", cx.listener(|ws, _, _, cx| {
                ws.sidebar_open = false;
                cx.notify();
            })));
        let nav = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(ui::nav_item("new", Icon::SquarePen, "New chat", false, cx.listener(|ws, _, w, cx| ws.new_chat(w, cx))))
            .child(ui::nav_item("nav-search", Icon::Search, "Search", self.panel == Panel::Search, cx.listener(|ws, _, w, cx| ws.page(Panel::Search, w, cx))))
            .child(ui::nav_item("nav-prs", Icon::GitPullRequest, "Pull requests", self.panel == Panel::PullRequests, cx.listener(|ws, _, w, cx| ws.page(Panel::PullRequests, w, cx))))
            .child(ui::nav_item("nav-customize", Icon::SlidersHorizontal, "Customize", in_customize, cx.listener(|ws, _, w, cx| ws.page(Panel::Settings, w, cx))));
        let projects_header = div()
            .px_1()
            .flex()
            .items_center()
            .justify_between()
            .child(
                ui::section_label(Icon::Folder, "Projects").child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(0x5a5d63))
                        .child("1"),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(ui::icon_button_xs("find-chat", Icon::Search, "Find chats", cx.listener(|ws, _, w, cx| ws.page(Panel::Search, w, cx))))
                    .child(ui::icon_button_xs("project", Icon::Plus, "Open project folder", cx.listener(|ws, _, _, cx| ws.pick_project(cx)))),
            );
        let project_row = div()
            .id("project-row")
            .role(Role::Button)
            .aria_label(SharedString::from(format!("Project {project_name}")))
            .aria_expanded(self.project_open)
            .tab_index(0)
            .cursor_pointer()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .py_1()
            .rounded(px(6.))
            .hover(|s| s.bg(rgb(theme::SECONDARY)))
            .child(
                icon(if self.project_open { Icon::ChevronDown } else { Icon::ChevronRight }, 12.)
                    .text_color(rgb(theme::MUTED_FOREGROUND)),
            )
            .child(icon(Icon::FolderOpen, 14.).text_color(rgb(theme::PRIMARY)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(project_name),
            )
            .on_click(cx.listener(|ws, _, _, cx| {
                ws.project_open = !ws.project_open;
                cx.notify();
            }));
        let splits = div().flex().flex_col().children(self.saved_layouts.iter().enumerate().map(|(index, group)| {
            chat_row(
                SharedString::from(format!("saved-split-{index}")),
                Icon::Columns2,
                format!("Split view with {} chats", group.ids().len()),
                String::new(),
                false,
                false,
                cx.listener(move |ws, _, w, cx| ws.reopen_group(index, w, cx)),
            )
        }));
        let body = div()
            .flex_1()
            .min_h_0()
            .px(px(6.))
            .py(px(6.))
            .flex()
            .flex_col()
            .child(nav)
            .child(div().my_2().h(px(1.)).bg(rgb(SECTION_LINE)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(projects_header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(project_row)
                            .when(self.project_open, |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .pl_3()
                                        .flex()
                                        .flex_col()
                                        .child(splits)
                                        .child(history),
                                )
                            }),
                    ),
            );
        let provider = self.chat(cx).options.lock().unwrap().provider.clone();
        let footer = div()
            .flex_none()
            .border_t_1()
            .border_color(rgb(SIDEBAR_LINE))
            .p(px(6.))
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                div()
                    .id("account")
                    .role(Role::Button)
                    .aria_label("Model & permissions")
                    .tab_index(0)
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p(px(6.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(rgb(theme::SECONDARY)))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(rgb(theme::CARD))
                            .border_1()
                            .border_color(rgb(theme::BORDER))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Z"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).line_height(px(16.)).child("Zest"))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .line_height(px(14.))
                                    .text_color(rgb(theme::MUTED_FOREGROUND))
                                    .child(provider),
                            ),
                    )
                    .child(icon(Icon::ChevronsUpDown, 14.).text_color(rgb(theme::MUTED_FOREGROUND)))
                    .on_click(cx.listener(|ws, _, w, cx| ws.page(Panel::Settings, w, cx))),
            )
            .child(ui::icon_button("usage", Icon::Gauge, "Usage", cx.listener(|ws, _, w, cx| ws.page(Panel::Usage, w, cx))));
        div()
            .w(px(theme::SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(theme::SIDEBAR))
            .border_r_1()
            .border_color(rgb(SIDEBAR_LINE))
            .child(top)
            .child(body)
            .child(footer)
            .into_any_element()
    }

    pub(super) fn header(&self, cx: &mut Context<Self>) -> Div {
        let chat = self.chat(cx);
        let thread = chat.thread.lock().unwrap();
        let title = thread.title.clone().unwrap_or_else(|| "New chat".into());
        let pinned = thread.pinned;
        let saved = !thread.messages.is_empty();
        let branch = thread
            .git_context
            .as_ref()
            .and_then(|g| g.branch.clone())
            .or_else(|| self.branch.clone());
        drop(thread);
        let project_name = self.project.path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let subtitle = match branch {
            Some(branch) => format!("{title} · {branch}"),
            None => title,
        };
        let mut actions = div().flex().items_center().gap(px(2.));
        if let Some(track) = self.extras.track_label() {
            actions = actions.child(ui::icon_button("music", Icon::Music, track, cx.listener(|ws, _, w, cx| ws.page(Panel::Extras, w, cx))));
        }
        if self.panel == Panel::Chat && saved {
            actions = actions
                .child(ui::icon_button("pin", if pinned { Icon::PinOff } else { Icon::Pin }, if pinned { "Unpin" } else { "Pin" }, cx.listener(|ws, _, _, cx| {
                    if let Err(e) = ws.chat(cx).edit(|t| Ok(t.set_pinned(!t.pinned))) {
                        ws.note = e.to_string();
                    }
                    ws.refresh(cx);
                })))
                .child(ui::icon_button("rename", Icon::Pencil, "Rename", cx.listener(|ws, _, w, cx| ws.page(Panel::Rename, w, cx))))
                .child(ui::icon_button("fork", Icon::GitFork, "Fork", cx.listener(|ws, _, w, cx| ws.fork(None, w, cx))));
        }
        if self.panel == Panel::Chat && self.layout.is_some() {
            actions = actions
                .child(ui::icon_button("split-right", Icon::Columns2, "Split right", cx.listener(|ws, _, w, cx| ws.add_pane(Direction::Across, w, cx))))
                .child(ui::icon_button("split-down", Icon::Rows2, "Split down", cx.listener(|ws, _, w, cx| ws.add_pane(Direction::Down, w, cx))))
                .child(ui::icon_button("zoom-pane", Icon::Maximize2, "Zoom pane", cx.listener(|ws, _, _, cx| ws.zoom_active(cx))))
                .child(ui::icon_button("minimize-pane", Icon::Minimize2, "Minimize pane", cx.listener(|ws, _, w, cx| ws.minimize_active(w, cx))))
                .child(ui::icon_button("restore-panes", Icon::RotateCcw, "Restore all panes", cx.listener(|ws, _, _, cx| {
                    if let Some(layout) = &mut ws.layout {
                        layout.restore();
                    }
                    cx.notify();
                })));
        }
        actions = actions
            .child(ui::icon_button(
                "split",
                if self.layout.is_some() { Icon::Square } else { Icon::Columns2 },
                if self.layout.is_some() { "Single view" } else { "Split" },
                cx.listener(|ws, _, w, cx| ws.split(w, cx)),
            ))
            .child(ui::icon_button("workbench", Icon::PanelRight, "Workbench", cx.listener(|ws, _, w, cx| ws.page(Panel::Changes, w, cx))))
            .child(ui::icon_button("open-folder", Icon::FolderOpen, "Open project folder", cx.listener(|ws, _, _, cx| ws.pick_project(cx))));
        div()
            .flex_none()
            .px_4()
            .py(px(10.))
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(theme::HEADER))
            .border_b_1()
            .border_color(rgb(0x1f2125))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).line_height(px(18.)).child(project_name))
                    .child(div().truncate().text_size(px(11.)).line_height(px(14.)).text_color(rgb(theme::MUTED_FOREGROUND)).child(subtitle)),
            )
            .child(actions)
    }

    /// A page such as Customize: centered column, title, optional tabs, then the panel body.
    pub(super) fn page_frame(&self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let group = self.page_group();
        let (title, subtitle) = match self.panel {
            Panel::Search => ("Search", "Find chats, saved text and panels."),
            Panel::PullRequests => ("Pull requests", "Pull requests linked to this project's chats."),
            Panel::Rename => ("Rename chat", "Type a new title and press Enter."),
            _ if group.is_some_and(|g| std::ptr::eq(g, CUSTOMIZE)) => (
                "Customize",
                "Models, permissions, skills, MCP servers and add-ons. Everything here stays on this computer.",
            ),
            _ => ("Workbench", "Changes, files, jobs, checkpoints, delegation and worktrees for this project."),
        };
        let heading = div()
            .flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_size(px(12.)).text_color(rgb(theme::MUTED_FOREGROUND)).child(subtitle)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::button("back", ui::Variant::Ghost, Some(Icon::ArrowLeft), "Back to chat", cx.listener(|ws, _, w, cx| ws.page(Panel::Chat, w, cx))))
                    .child(kbd("Esc")),
            );
        let tabs = group.map(|group| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .pb_3()
                .border_b_1()
                .border_color(rgb(0x1f2125))
                .children(group.iter().map(|&(panel, label, glyph)| {
                    let active = panel == self.panel;
                    ui::button(
                        SharedString::from(format!("tab-{label}")),
                        if active { ui::Variant::Secondary } else { ui::Variant::Ghost },
                        Some(glyph),
                        label,
                        cx.listener(move |ws, _, w, cx| ws.page(panel, w, cx)),
                    )
                    .when(active, |d| d.text_color(rgb(theme::FOREGROUND)))
                }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .justify_center()
            .bg(rgb(theme::CANVAS))
            .child(
                div()
                    .w_full()
                    .max_w(px(880.))
                    .h_full()
                    .px_6()
                    .pt_6()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(heading)
                    .when_some(tabs, |d, tabs| d.child(tabs))
                    .child(body),
            )
            .into_any_element()
    }
}

/// The chat area as a rounded, bordered canvas inset below the header.
pub(super) fn canvas_frame(content: AnyElement) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .mt(px(6.))
        .mb_2()
        .mr_2()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(alpha(theme::FOREGROUND, 0.14))
        .bg(rgb(theme::CANVAS))
        .overflow_hidden()
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::age;

    #[::core::prelude::v1::test]
    fn ages_read_like_the_react_sidebar() {
        assert_eq!(age(100, 101), "now");
        assert_eq!(age(100, 145), "45s");
        assert_eq!(age(0, 6 * 60), "6m");
        assert_eq!(age(0, 8 * 3600), "8h");
        assert_eq!(age(0, 2 * 86400 + 5), "2d");
        assert_eq!(age(200, 100), "now");
    }
}
