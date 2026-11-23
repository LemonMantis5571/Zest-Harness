//! The command palette (Ctrl+K): one field that finds actions, pages and chats. Up and Down
//! move the selection, Enter runs it, Escape or a click outside closes the palette. Typing a
//! word that matches no title still offers "Search messages for …", which opens the Search
//! page with that text.

use super::*;
use crate::composer::{Composer, ComposerEvent};
use crate::theme::{self, Icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    NewChat,
    OpenFolder,
    Split,
    Zoom,
    Minimize,
    Back,
    Forward,
    Fork,
    Rename,
    TogglePin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Act(Act),
    Page(Panel),
    Chat(String),
    SearchMessages(String),
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) shortcut: Option<&'static str>,
    pub(crate) group: &'static str,
    pub(crate) target: Target,
}

/// The actions the palette offers, with what they do and their shortcut.
const ACTIONS: &[(Act, &str, &str, Option<&str>)] = &[
    (Act::NewChat, "New chat", "Start a fresh conversation in this project", Some("Ctrl+N")),
    (Act::OpenFolder, "Open folder", "Open another project in its own window", Some("Ctrl+O")),
    (Act::Split, "Split view", "Show chats side by side, or leave split view", Some("Ctrl+\\")),
    (Act::Zoom, "Zoom pane", "Show the active pane alone, or restore the split", Some("Ctrl+Shift+Enter")),
    (Act::Minimize, "Minimize pane", "Hide the active pane from the split", Some("Ctrl+Shift+H")),
    (Act::Back, "Go back", "Open the previous chat or page", None),
    (Act::Forward, "Go forward", "Open the next chat or page", None),
    (Act::Fork, "Fork this chat", "Continue this conversation in a new chat", None),
    (Act::Rename, "Rename this chat", "Change the chat's title", None),
    (Act::TogglePin, "Pin or unpin this chat", "Keep the chat at the top of the sidebar", None),
];

/// How well `query` matches `text`: lower is better, `None` is no match. A prefix beats the
/// start of a word, which beats anywhere in the text, which beats the letters in order.
pub(crate) fn score(query: &str, text: &str) -> Option<u32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if text.starts_with(&query) {
        return Some(0);
    }
    if let Some(at) = text.find(&query) {
        let word_start = text[..at].ends_with(|c: char| !c.is_alphanumeric());
        return Some(if word_start { 1 } else { 2 });
    }
    let mut rest = text.chars();
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|q| rest.by_ref().any(|t| t == q))
        .then_some(3)
}

/// What the palette lists for `query`: actions, then pages, then chats (at most eight), each
/// group ordered by match quality, then "Search messages for …" when there is a query.
pub(crate) fn items(query: &str, pages: &[(Panel, &str)], chats: &[(String, String)]) -> Vec<Item> {
    fn ranked(mut found: Vec<(u32, Item)>) -> Vec<Item> {
        found.sort_by_key(|(rank, _)| *rank);
        found.into_iter().map(|(_, item)| item).collect()
    }
    let actions = ACTIONS
        .iter()
        .filter_map(|&(act, label, detail, shortcut)| {
            // Descriptions match only on the typed text itself; letters scattered through a
            // sentence match almost anything.
            let rank = score(query, label)
                .or_else(|| score(query, detail).filter(|r| *r <= 2).map(|r| r + 4))?;
            Some((rank, Item { label: label.into(), detail: detail.into(), shortcut, group: "Actions", target: Target::Act(act) }))
        })
        .collect();
    let pages = pages
        .iter()
        .filter(|(page, _)| *page != Panel::Search)
        .filter_map(|&(page, name)| {
            let rank = score(query, name)?;
            Some((rank, Item { label: name.into(), detail: "Open page".into(), shortcut: None, group: "Pages", target: Target::Page(page) }))
        })
        .collect();
    let chats: Vec<(u32, Item)> = chats
        .iter()
        .filter_map(|(id, title)| {
            let rank = score(query, title)?;
            Some((rank, Item { label: title.clone(), detail: "Open chat".into(), shortcut: None, group: "Chats", target: Target::Chat(id.clone()) }))
        })
        .collect();
    let mut all = ranked(actions);
    all.extend(ranked(pages));
    all.extend(ranked(chats).into_iter().take(8));
    let query = query.trim();
    if !query.is_empty() {
        all.push(Item {
            label: format!("Search messages for \u{201c}{query}\u{201d}"),
            detail: "Find chats whose messages contain this text".into(),
            shortcut: None,
            group: "Search",
            target: Target::SearchMessages(query.to_string()),
        });
    }
    all
}

pub(crate) struct Palette {
    input: Entity<Composer>,
    selected: usize,
    _subscriptions: [Subscription; 2],
}

impl Workspace {
    /// Opens the palette, or closes it when it is open.
    pub(super) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        let input = cx.new(|cx| {
            let mut field = Composer::new(cx);
            field.placeholder = "Type a command, page or chat";
            field.list_nav = true;
            field
        });
        let changed = cx.observe(&input, |ws, _, cx| {
            if let Some(palette) = ws.palette.as_mut() {
                palette.selected = 0;
            }
            cx.notify();
        });
        let events = cx.subscribe_in(&input, window, |ws, _, ev: &ComposerEvent, window, cx| match ev {
            ComposerEvent::Send { .. } => ws.run_palette_selection(window, cx),
            ComposerEvent::Stop => ws.toggle_palette(window, cx),
            ComposerEvent::Navigate(step) => {
                let count = ws.palette_items(cx).len();
                if let Some(palette) = ws.palette.as_mut().filter(|_| count > 0) {
                    let next = palette.selected as i64 + *step as i64;
                    palette.selected = next.rem_euclid(count as i64) as usize;
                    cx.notify();
                }
            }
        });
        window.focus(&input.read(cx).focus.clone(), cx);
        self.palette = Some(Palette { input, selected: 0, _subscriptions: [changed, events] });
        cx.notify();
    }

    fn palette_items(&self, cx: &App) -> Vec<Item> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        let query = palette.input.read(cx).text.clone();
        let chats: Vec<(String, String)> = self
            .history
            .iter()
            .map(|row| (row.id.clone(), chat_title(row)))
            .collect();
        items(&query, PAGES, &chats)
    }

    fn run_palette_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.palette.as_ref().map(|p| p.selected).unwrap_or(0);
        if let Some(item) = self.palette_items(cx).into_iter().nth(selected) {
            self.run_palette(item.target, window, cx);
        }
    }

    fn run_palette(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        window.focus(&self.focus, cx);
        match target {
            Target::Act(act) => match act {
                Act::NewChat => self.new_chat(window, cx),
                Act::OpenFolder => self.pick_project(cx),
                Act::Split => self.split(window, cx),
                Act::Zoom => self.zoom_active(cx),
                Act::Minimize => self.minimize_active(window, cx),
                Act::Back => self.go(-1, window, cx),
                Act::Forward => self.go(1, window, cx),
                Act::Fork => self.fork(None, window, cx),
                Act::Rename => self.page(Panel::Rename, window, cx),
                Act::TogglePin => {
                    if let Err(e) = self.chat(cx).edit(|t| Ok(t.set_pinned(!t.pinned))) {
                        self.toast(ToastKind::Error, e.to_string(), cx);
                    }
                    self.refresh(cx);
                }
            },
            Target::Page(page) => self.page(page, window, cx),
            Target::Chat(id) => self.open(&id, window, cx),
            Target::SearchMessages(text) => {
                self.page(Panel::Search, window, cx);
                self.filter.update(cx, |field, cx| {
                    field.sel = text.len()..text.len();
                    field.text = text;
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    /// The palette over the window: a dimmed backdrop and a card with the field and results.
    pub(super) fn palette_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let items = self.palette_items(cx);
        let mut list = div().id("palette-results").role(Role::ListBox).aria_label("Results").flex().flex_col().max_h(px(420.)).overflow_y_scroll();
        let mut group = "";
        for (index, item) in items.iter().enumerate() {
            if item.group != group {
                group = item.group;
                list = list.child(
                    div().px_3().pt_2().pb_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(theme::muted_foreground())).child(group),
                );
            }
            let selected = index == palette.selected;
            let target = item.target.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("palette-item-{index}")))
                    .role(Role::ListBoxOption)
                    .aria_label(SharedString::from(item.label.clone()))
                    .aria_selected(selected)
                    .px_3()
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(rgb(theme::selected_row())))
                    .hover(|d| d.bg(rgb(theme::selected_row())))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(item.label.clone()))
                            .child(div().truncate().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(item.detail.clone())),
                    )
                    .when_some(item.shortcut, |d, keys| {
                        d.child(div().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(keys))
                    })
                    .on_click(cx.listener(move |ws, _, w, cx| ws.run_palette(target.clone(), w, cx))),
            );
        }
        if items.is_empty() {
            list = list.child(crate::ui::hint("Nothing matches."));
        }
        let card = div()
            .id("palette")
            .role(Role::Dialog)
            .aria_label("Command palette")
            .w(px(560.))
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::popover()))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(theme::icon(Icon::Search, 14.))
                    .child(div().flex_1().child(palette.input.clone())),
            )
            .child(list);
        Some(
            div()
                .id("palette-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .pt(px(80.))
                .bg(gpui::rgba(0x0000_0055))
                .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, w, cx| ws.toggle_palette(w, cx)))
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{items, score, Act, Panel, Target};

    #[::core::prelude::v1::test]
    fn prefix_beats_word_start_beats_inside_beats_letters_in_order() {
        assert_eq!(score("new", "New chat"), Some(0));
        assert_eq!(score("chat", "New chat"), Some(1));
        assert_eq!(score("hat", "New chat"), Some(2));
        assert_eq!(score("nc", "New chat"), Some(3));
        assert_eq!(score("xyz", "New chat"), None);
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[::core::prelude::v1::test]
    fn results_are_actions_then_pages_then_chats_then_message_search() {
        let pages = [(Panel::Appearance, "Appearance"), (Panel::Search, "Find chats"), (Panel::Changes, "Changes")];
        let chats = vec![("a".to_string(), "Change the login page".to_string())];
        let found = items("chan", &pages, &chats);
        let groups: Vec<_> = found.iter().map(|i| i.group).collect();
        // Rename matches through its description ("Change the chat's title").
        assert_eq!(groups, ["Actions", "Pages", "Chats", "Search"]);
        assert_eq!(found[0].target, Target::Act(Act::Rename));
        assert_eq!(found[1].target, Target::Page(Panel::Changes));
        assert_eq!(found[2].target, Target::Chat("a".into()));
        assert_eq!(found[3].target, Target::SearchMessages("chan".into()));
        assert!(found.iter().all(|i| i.target != Target::Page(Panel::Search)), "Find chats is the palette's own job");
    }

    #[::core::prelude::v1::test]
    fn an_empty_query_lists_every_action_and_no_message_search() {
        let found = items("", &[], &[]);
        assert!(found.iter().any(|i| i.target == Target::Act(Act::NewChat) && i.shortcut == Some("Ctrl+N")));
        assert!(found.iter().all(|i| i.group == "Actions"));
    }

    #[::core::prelude::v1::test]
    fn at_most_eight_chats_are_listed() {
        let chats: Vec<_> = (0..20).map(|n| (n.to_string(), format!("Chat {n}"))).collect();
        let found = items("chat", &[], &chats);
        assert_eq!(found.iter().filter(|i| i.group == "Chats").count(), 8);
    }
}
