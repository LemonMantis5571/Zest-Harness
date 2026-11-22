//! The workspace frame drawn like the React UI: sidebar, chat header and page frame.

use super::*;
use crate::theme::{self, Icon, alpha, icon};
use crate::ui;

/// Settings-type panels, shown as tabs under Customize (the React UI's CustomizePanel).
pub(super) const CUSTOMIZE: &[(Panel, &str, Icon)] = &[
    (Panel::Appearance, "Appearance", Icon::Palette),
    (Panel::ChatView, "Chat view", Icon::MessageSquare),
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
                .text_color(rgb(theme::foreground()))
                .when(selected, |d| {
                    d.bg(rgb(theme::selected_row())).child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(4.))
                            .bottom(px(4.))
                            .w(px(2.))
                            .rounded_full()
                            .bg(rgb(theme::primary())),
                    )
                })
                .when(!selected, |d| d.hover(|s| s.bg(rgb(theme::secondary()))))
                .focus_visible(|s| s.bg(rgb(theme::accent())))
                .child(
                    icon(glyph, 16.)
                        .text_color(if busy { rgb(theme::primary()) } else { rgb(theme::muted_foreground()) }),
                )
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_none()
                        .pr_1()
                        .text_size(px(10.))
                        .text_color(rgb(theme::muted_foreground()))
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
        .border_color(rgb(theme::border()))
        .text_size(px(10.))
        .text_color(rgb(theme::muted_foreground()))
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
                .bg(rgb(theme::sidebar()))
                .border_r_1()
                .border_color(rgb(theme::sidebar_line()))
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
                            let needs_input = ws.chats.get(&row.id).is_some_and(|c| !c.read(cx).cards.is_empty());
                            let pr = row.git_context.as_ref().is_some_and(|g| g.pull_request.is_some());
                            let glyph = if theme::compact_sidebar() {
                                // needs input > working > pull request > idle
                                if needs_input {
                                    Icon::CircleAlert
                                } else if busy {
                                    Icon::LoaderCircle
                                } else if pr {
                                    Icon::GitPullRequest
                                } else {
                                    Icon::Circle
                                }
                            } else if busy {
                                Icon::LoaderCircle
                            } else if row.pinned {
                                Icon::Pin
                            } else if pr {
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
            .border_color(rgb(theme::sidebar_line()))
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
                        .text_color(rgb(crate::theme::faint()))
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
            .hover(|s| s.bg(rgb(theme::secondary())))
            .child(
                icon(if self.project_open { Icon::ChevronDown } else { Icon::ChevronRight }, 12.)
                    .text_color(rgb(theme::muted_foreground())),
            )
            .child(icon(Icon::FolderOpen, 14.).text_color(rgb(theme::primary())))
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
        // Compact list: the project name is the only heading tier, with New chat always visible.
        let compact_heading = div()
            .px(px(6.))
            .h(px(24.))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::muted_foreground()))
                    .child(
                        self.project.path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    ),
            )
            .child(ui::icon_button_xs("compact-new-chat", Icon::Plus, "New chat in this project", cx.listener(|ws, _, w, cx| ws.new_chat(w, cx))));
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
            .child(div().my_2().h(px(1.)).bg(rgb(theme::section_line())))
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
                            .when(!theme::compact_sidebar(), |d| d.child(project_row))
                            .when(theme::compact_sidebar(), |d| d.child(compact_heading))
                            .when(self.project_open || theme::compact_sidebar(), |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .when(!theme::compact_sidebar(), |d| d.pl_3())
                                        .flex()
                                        .flex_col()
                                        .child(splits)
                                        .child(history),
                                )
                            }),
                    )
                    .children(self.recent_section(cx)),
            );
        let provider = self.chat(cx).options.lock().unwrap().provider.clone();
        let footer = div()
            .flex_none()
            .border_t_1()
            .border_color(rgb(theme::sidebar_line()))
            .p(px(6.))
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                div()
                    .id("account")
                    .role(Role::Button)
                    .aria_label("Your profile")
                    .tab_index(0)
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p(px(6.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(rgb(theme::secondary())))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(rgb(theme::card()))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .map(|d| match &self.profile.avatar {
                                Some(image) => d.child(img(image.clone()).size_full().object_fit(ObjectFit::Cover)),
                                None => d.child(
                                    self.profile.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "Z".into()),
                                ),
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).line_height(px(16.)).child(if self.profile.name.is_empty() { "Zest".to_string() } else { self.profile.name.clone() }))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .line_height(px(14.))
                                    .text_color(rgb(theme::muted_foreground()))
                                    .child(provider),
                            ),
                    )
                    .child(icon(Icon::ChevronsUpDown, 14.).text_color(rgb(theme::muted_foreground())))
                    .on_click(cx.listener(|ws, _, w, cx| ws.page(Panel::Profile, w, cx))),
            )
            .child(ui::icon_button("usage", Icon::Gauge, "Usage", cx.listener(|ws, _, w, cx| ws.page(Panel::Usage, w, cx))));
        div()
            .w(px(theme::SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(theme::sidebar()))
            .border_r_1()
            .border_color(rgb(theme::sidebar_line()))
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
            .bg(rgb(theme::header()))
            .border_b_1()
            .border_color(rgb(crate::theme::canvas_line()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).line_height(px(18.)).child(project_name))
                    .child(div().truncate().text_size(px(11.)).line_height(px(14.)).text_color(rgb(theme::muted_foreground())).child(subtitle)),
            )
            .child(actions)
    }

    /// A page such as Customize: centered column, title, optional tabs, then the panel body.
    pub(super) fn page_frame(&self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let group = self.page_group();
        let (title, subtitle) = match self.panel {
            Panel::Search => ("Search", "Find chats, saved text and panels."),
            Panel::Profile => ("Your profile", "Your activity in Zest on this computer. Press Enter in the name field to save it."),
            Panel::PullRequests => ("Pull requests", "Pull requests linked to this project's chats."),
            Panel::Rename => ("Rename chat", "Type a new title and press Enter."),
            _ if group.is_some_and(|g| std::ptr::eq(g, CUSTOMIZE)) => (
                "Customize",
                "Appearance, models, permissions, skills, MCP servers and add-ons. Everything here stays on this computer.",
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
                    .child(div().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(subtitle)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::button("back", ui::Variant::Ghost, Some(Icon::ArrowLeft), "Back to chat", cx.listener(|ws, _, w, cx| ws.page(Panel::Chat, w, cx))))
                    .child(kbd("Esc")),
            );
        // Tabs follow UI SDK's tabs behavior: only the selected tab is a tab stop, and
        // arrow keys, Home and End move the selection and focus together.
        let tabs = group.map(|group| {
            let count = group.len();
            div()
                .id("page-tabs")
                .role(Role::TabList)
                .aria_orientation(accesskit::Orientation::Horizontal)
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .pb_3()
                .border_b_1()
                .border_color(rgb(crate::theme::canvas_line()))
                .children(group.iter().enumerate().map(|(index, &(panel, label, glyph))| {
                    let active = panel == self.panel;
                    ui::button(
                        SharedString::from(format!("tab-{label}")),
                        if active { ui::Variant::Secondary } else { ui::Variant::Ghost },
                        Some(glyph),
                        label,
                        cx.listener(move |ws, _, w, cx| ws.page(panel, w, cx)),
                    )
                    .role(Role::Tab)
                    .aria_selected(active)
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(count)
                    .track_focus(&self.tab_focus[index])
                    .tab_stop(active)
                    .when(active, |d| d.text_color(rgb(theme::foreground())))
                    .on_key_down(cx.listener(move |ws, event: &KeyDownEvent, window, cx| {
                        let target = match event.keystroke.key.as_str() {
                            "left" => (index + count - 1) % count,
                            "right" => (index + 1) % count,
                            "home" => 0,
                            "end" => count - 1,
                            _ => return,
                        };
                        cx.stop_propagation();
                        ws.page(group[target].0, window, cx);
                        window.focus(&ws.tab_focus[target], cx);
                    }))
                }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .justify_center()
            .bg(rgb(theme::canvas()))
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

impl Workspace {
    /// Customize > Appearance: theme and font, applied at once and saved per machine.
    pub(super) fn appearance_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = theme::palette().id;
        let themes = div().flex().flex_wrap().gap_3().children(theme::THEMES.iter().map(|palette| {
            let selected = palette.id == active;
            let id = palette.id;
            let swatch = |color: u32| {
                div()
                    .size(px(18.))
                    .rounded_full()
                    .border_1()
                    .border_color(alpha(theme::foreground(), 0.15))
                    .bg(rgb(color))
            };
            div()
                .id(SharedString::from(format!("theme-{id}")))
                .role(Role::Button)
                .aria_label(SharedString::from(if selected {
                    format!("Theme: {}, current", palette.name)
                } else {
                    format!("Theme: {}", palette.name)
                }))
                .tab_index(0)
                .cursor_pointer()
                .w(px(240.))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(8.))
                .border_1()
                .border_color(if selected { rgb(theme::primary()) } else { rgb(theme::soft_border()) })
                .bg(rgb(theme::card()))
                .hover(|s| s.bg(rgb(theme::secondary())))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(swatch(palette.background))
                        .child(swatch(palette.sidebar))
                        .child(swatch(palette.card))
                        .child(swatch(palette.primary)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(palette.name)
                        .when(selected, |d| d.child(icon(Icon::Check, 14.).text_color(rgb(theme::primary()))))
                        .when(!palette.dark, |d| d.child(ui::pill(None, "Light"))),
                )
                .child(ui::hint(palette.description))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    theme::set_theme(id);
                    ws.save_appearance(window, cx);
                }))
        }));
        let current_face = theme::face().id;
        let faces = div().flex().flex_col().gap(px(2.)).children(theme::available_faces(cx).into_iter().map(|face| {
            let selected = face.id == current_face;
            let id = face.id;
            div()
                .id(SharedString::from(format!("font-{id}")))
                .role(Role::Button)
                .aria_label(SharedString::from(if selected {
                    format!("Font: {}, current", face.name)
                } else {
                    format!("Font: {}", face.name)
                }))
                .tab_index(0)
                .cursor_pointer()
                .h(px(36.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.))
                .when(selected, |d| d.bg(alpha(theme::primary(), 0.18)))
                .hover(|s| s.bg(rgb(theme::secondary())))
                .child(div().flex_1().font_family(face.family).text_size(px(14.)).child(face.name))
                .when(selected, |d| d.child(icon(Icon::Check, 14.).text_color(rgb(theme::primary()))))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    theme::set_face(id);
                    ws.save_appearance(window, cx);
                }))
        }));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(ui::section_title("Theme"))
            .child(themes)
            .child(ui::section_title("Typography"))
            .child(ui::hint("Geist ships with Zest. Inter, Plus Jakarta Sans, JetBrains Mono and Fira Code appear here when installed."))
            .child(div().max_w(px(420.)).child(faces))
            .into_any_element()
    }

    /// Customize > Chat view: the history layout and how streaming replies appear.
    pub(super) fn chat_view_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let compact = theme::compact_sidebar();
        let option = |id: &'static str, title: &'static str, hint: &'static str, glyph: Icon, selected: bool, choose: bool| {
            div()
                .id(id)
                .role(Role::RadioButton)
                .aria_label(SharedString::from(title))
                .aria_toggled(if selected { accesskit::Toggled::True } else { accesskit::Toggled::False })
                .tab_index(0)
                .cursor_pointer()
                .w(px(240.))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(8.))
                .border_1()
                .border_color(if selected { rgb(theme::primary()) } else { rgb(theme::soft_border()) })
                .bg(rgb(theme::card()))
                .hover(|s| s.bg(rgb(theme::secondary())))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon(glyph, 16.))
                        .child(title)
                        .when(selected, |d| d.child(icon(Icon::Check, 14.).text_color(rgb(theme::primary())))),
                )
                .child(ui::hint(hint))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    theme::set_compact_sidebar(choose);
                    ws.save_appearance(window, cx);
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(ui::section_title("Chat history"))
            .child(
                div()
                    .id("chat-history-view")
                    .role(Role::RadioGroup)
                    .aria_label("Chat history visualization")
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(option("view-project", "Project tree", "Projects expand into their chats, with ages and status icons.", Icon::Folder, !compact, false))
                    .child(option("view-compact", "Compact list", "One line per chat with a single status mark.", Icon::ListTodo, compact, true)),
            )
            .child(ui::section_title("Response display"))
            .child(div().max_w(px(560.)).child(ui::switch_row(
                "block-streaming",
                "Stream complete blocks",
                "Show a reply block by block as each paragraph, list or code block completes.",
                theme::block_streaming(),
                cx.listener(|ws, on: &bool, window, cx| {
                    theme::set_block_streaming(*on);
                    ws.save_appearance(window, cx);
                }),
            )))
            .into_any_element()
    }

    fn save_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = theme::save_appearance() {
            self.note = error;
        }
        window.refresh();
        cx.refresh_windows();
        cx.notify();
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
        .border_color(alpha(theme::foreground(), 0.14))
        .bg(rgb(theme::canvas()))
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
