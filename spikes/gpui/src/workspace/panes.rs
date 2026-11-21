use super::*;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Direction {
    Across,
    Down,
}

pub(super) struct Layout {
    root: Pane,
    minimized: HashSet<String>,
    zoom: Option<String>,
}

enum Pane {
    Chat(String),
    Split {
        direction: Direction,
        fraction: f32,
        first: Box<Pane>,
        second: Box<Pane>,
    },
}

impl Pane {
    fn find(&mut self, chat: &str) -> Option<&mut Pane> {
        match self {
            Pane::Chat(id) if id == chat => Some(self),
            Pane::Chat(_) => None,
            Pane::Split { first, second, .. } => first.find(chat).or_else(|| second.find(chat)),
        }
    }

    fn leaves(&self, ids: &mut Vec<String>) {
        match self {
            Pane::Chat(id) => ids.push(id.clone()),
            Pane::Split { first, second, .. } => {
                first.leaves(ids);
                second.leaves(ids);
            }
        }
    }

    fn remove(self, chat: &str) -> Option<Self> {
        match self {
            Pane::Chat(id) => (id != chat).then_some(Pane::Chat(id)),
            Pane::Split {
                direction,
                fraction,
                first,
                second,
            } => match (first.remove(chat), second.remove(chat)) {
                (Some(a), Some(b)) => Some(Pane::Split {
                    direction,
                    fraction,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (a, b) => a.or(b),
            },
        }
    }
}

impl Layout {
    pub(super) fn new(first: String) -> Self {
        Self {
            root: Pane::Chat(first),
            minimized: HashSet::new(),
            zoom: None,
        }
    }

    pub(super) fn ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.root.leaves(&mut ids);
        ids
    }

    pub(super) fn visible(&self, id: &str) -> bool {
        self.zoom.as_deref().is_none_or(|zoom| zoom == id) && !self.minimized.contains(id)
    }

    pub(super) fn reveal(&mut self, id: &str) {
        if !self.visible(id) {
            self.zoom = None;
            self.minimized.remove(id);
        }
    }

    pub(super) fn open(&mut self, current: &str, next: &str) {
        if self.root.find(next).is_none() {
            if let Some(leaf) = self.root.find(current) {
                *leaf = Pane::Chat(next.to_owned());
            }
            self.minimized.remove(current);
            if self.zoom.as_deref() == Some(current) {
                self.zoom = Some(next.into());
            }
        }
        self.reveal(next);
    }

    fn split(&mut self, current: &str, next: &str, direction: Direction) {
        self.zoom = None;
        self.minimized.remove(current);
        if let Some(leaf) = self.root.find(current) {
            *leaf = Pane::Split {
                direction,
                fraction: 0.5,
                first: Box::new(Pane::Chat(current.into())),
                second: Box::new(Pane::Chat(next.into())),
            };
        }
    }

    fn zoom(&mut self, id: &str) {
        self.minimized.remove(id);
        self.zoom = (self.zoom.as_deref() != Some(id)).then(|| id.to_string());
    }

    pub(super) fn restore(&mut self) {
        self.zoom = None;
        self.minimized.clear();
    }

    fn minimize(&mut self, id: &str) -> Option<String> {
        if self.minimized.remove(id) {
            self.zoom = None;
            return Some(id.to_string());
        }
        let next = self
            .ids()
            .into_iter()
            .find(|other| other != id && !self.minimized.contains(other))?;
        self.zoom = None;
        self.minimized.insert(id.into());
        Some(next)
    }

    fn resize(&mut self, path: &str, fraction: f32) {
        let mut node = &mut self.root;
        for step in path.bytes() {
            let Pane::Split { first, second, .. } = node else {
                return;
            };
            node = if step == b'L' { first } else { second };
        }
        if let Pane::Split {
            fraction: current, ..
        } = node
        {
            *current = fraction.clamp(0.30, 0.70);
        }
    }
}

struct Divider(String);

impl Workspace {
    pub(super) fn reopen_group(&mut self, index: usize, w: &mut Window, cx: &mut Context<Self>) {
        if index >= self.saved_layouts.len() {
            return;
        }
        let group = self.saved_layouts.remove(index);
        let Some(next) = group.ids().into_iter().find(|id| group.visible(id)) else {
            return;
        };
        if let Some(previous) = self.layout.replace(group) {
            self.saved_layouts.push(previous);
        }
        self.open(&next, w, cx);
    }

    pub(super) fn add_pane(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.active.clone();
        let mut layout = self
            .layout
            .take()
            .unwrap_or_else(|| Layout::new(previous.clone()));
        self.new_chat(window, cx);
        layout.split(&previous, &self.active, direction);
        self.layout = Some(layout);
        cx.notify();
    }

    pub(super) fn close_pane(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut layout) = self.layout.take() else {
            return;
        };
        let Some(remaining) = layout.root.remove(id) else {
            return;
        };
        layout.root = remaining;
        layout.minimized.remove(id);
        if layout.zoom.as_deref() == Some(id) {
            layout.zoom = None;
        }
        let ids = layout.ids();
        let next = ids
            .iter()
            .find(|candidate| **candidate == self.active)
            .or_else(|| ids.iter().find(|candidate| layout.visible(candidate)))
            .unwrap_or(&ids[0])
            .clone();
        layout.reveal(&next);
        if ids.len() > 1 {
            self.layout = Some(layout);
        }
        self.open(&next, window, cx);
    }

    pub(super) fn zoom_active(&mut self, cx: &mut Context<Self>) {
        if let Some(layout) = &mut self.layout {
            layout.zoom(&self.active);
            cx.notify();
        }
    }

    pub(super) fn minimize_active(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.chats[&self.active].read(cx).cards.is_empty() {
            return;
        }
        if let Some(next) = self
            .layout
            .as_mut()
            .and_then(|layout| layout.minimize(&self.active))
        {
            self.open(&next, w, cx);
        }
    }

    pub(super) fn panes(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.layout {
            None => div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.chats[&self.active].clone())
                .into_any_element(),
            Some(layout) => self.render_pane(&layout.root, "", layout, cx),
        }
    }

    fn render_pane(
        &self,
        node: &Pane,
        path: &str,
        layout: &Layout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            Pane::Chat(id) => {
                let chat = self.chats[id].clone();
                let title = chat
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
                let target = id.clone();
                let hidden = !layout.visible(id);
                let label = if hidden {
                    let root = chat.read(cx);
                    let status = if !root.cards.is_empty() {
                        "Needs input"
                    } else if root.turn_active {
                        "Running"
                    } else {
                        "Idle"
                    };
                    format!("Restore {title}, {status}")
                } else {
                    title.clone()
                };
                let active = self.active == *id;
                let header = div()
                    .id(SharedString::from(format!("select-{id}")))
                    .role(Role::Button)
                    .aria_label(SharedString::from(label.clone()))
                    .tab_index(0)
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(if active { crate::theme::foreground() } else { crate::theme::muted_foreground() }))
                    .hover(|s| s.text_color(rgb(crate::theme::foreground())))
                    .when(active, |d| d.child(div().size(px(6.)).flex_none().rounded_full().bg(rgb(crate::theme::primary()))))
                    .child(div().min_w_0().truncate().child(label))
                    .on_click(cx.listener(move |ws, _, w, cx| ws.open(&target, w, cx)));
                let mut region = div()
                    .id(SharedString::from(format!("pane-{id}")))
                    .role(Role::Region)
                    .aria_label(title)
                    .size_full()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col();
                if hidden {
                    return region
                        .bg(crate::theme::alpha(crate::theme::foreground(), 0.04))
                        .border_1()
                        .border_color(rgb(crate::theme::canvas_line()))
                        .child(div().h(px(32.)).flex().child(header))
                        .into_any_element();
                }
                let target = id.clone();
                region = region.child(
                    div()
                        .h(px(32.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .pr_1()
                        .border_b_1()
                        .border_color(rgb(crate::theme::canvas_line()))
                        .child(header)
                        .child(crate::ui::icon_button_xs(
                            SharedString::from(format!("close-{id}")),
                            crate::theme::Icon::X,
                            "Close pane",
                            cx.listener(move |ws, _, w, cx| ws.close_pane(&target, w, cx)),
                        )),
                );
                region
                    .child(div().flex_1().min_h_0().child(chat))
                    .into_any_element()
            }
            Pane::Split {
                direction,
                fraction,
                first,
                second,
            } => {
                let across = *direction == Direction::Across;
                let minimized = |branch: &Pane| {
                    let mut ids = Vec::new();
                    branch.leaves(&mut ids);
                    ids.iter().all(|id| !layout.visible(id))
                };
                let first_hidden = minimized(first);
                let second_hidden = minimized(second);
                let wrap = |content: AnyElement, hidden, adjacent_hidden, ratio| {
                    div()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .when(hidden, |d| {
                            d.flex_basis(px(if across { 94. } else { 34. }))
                                .flex_shrink_0()
                        })
                        .when(!hidden && adjacent_hidden, |d| d.flex_1())
                        .when(!hidden && !adjacent_hidden, |d| {
                            d.flex_basis(relative(ratio))
                        })
                        .child(content)
                };
                let first = wrap(
                    self.render_pane(first, &format!("{path}L"), layout, cx),
                    first_hidden,
                    second_hidden,
                    *fraction,
                );
                let second = wrap(
                    self.render_pane(second, &format!("{path}R"), layout, cx),
                    second_hidden,
                    first_hidden,
                    1. - fraction,
                );
                let address = path.to_string();
                let keyboard_path = address.clone();
                let reset_path = address.clone();
                let current_fraction = *fraction;
                let divider = div()
                    .id(SharedString::from(format!("divider-{path}")))
                    .role(Role::Splitter)
                    .aria_label("Resize split panes")
                    .aria_value(format!("{:.0}%", fraction * 100.))
                    .tab_index(0)
                    .group("divider")
                    .flex()
                    .justify_center()
                    .items_center()
                    .flex_shrink_0()
                    .when(across, |d| d.w(px(5.)).cursor_col_resize().flex_row())
                    .when(!across, |d| d.h(px(5.)).cursor_row_resize().flex_col())
                    .child(
                        div()
                            .bg(rgb(crate::theme::border()))
                            .group_hover("divider", |s| s.bg(rgb(crate::theme::primary())))
                            .when(across, |d| d.w(px(1.)).h_full())
                            .when(!across, |d| d.h(px(1.)).w_full()),
                    )
                    .on_key_down(cx.listener(move |ws, event: &KeyDownEvent, _, cx| {
                        let value = match event.keystroke.key.as_str() {
                            "home" => 0.30,
                            "end" => 0.70,
                            "left" if across => current_fraction - 0.02,
                            "right" if across => current_fraction + 0.02,
                            "up" if !across => current_fraction - 0.02,
                            "down" if !across => current_fraction + 0.02,
                            _ => return,
                        };
                        if let Some(layout) = &mut ws.layout {
                            layout.resize(&keyboard_path, value);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |ws, event: &ClickEvent, _, cx| {
                        if event.click_count() == 2
                            && let Some(layout) = &mut ws.layout
                        {
                            layout.resize(&reset_path, 0.5);
                            cx.notify();
                        }
                    }))
                    .on_drag(Divider(address.clone()), |_, _, _, cx| cx.new(|_| Empty));
                div()
                    .id(SharedString::from(format!("split-{path}")))
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .when(!across, |d| d.flex_col())
                    .child(first)
                    .when(!first_hidden && !second_hidden, |d| d.child(divider))
                    .child(second)
                    .on_drag_move::<Divider>(cx.listener(
                        move |ws, event: &DragMoveEvent<Divider>, _, cx| {
                            if event.drag(cx).0 != address {
                                return;
                            }
                            let relative = event.event.position - event.bounds.origin;
                            let (offset, length) = if across {
                                (relative.x, event.bounds.size.width)
                            } else {
                                (relative.y, event.bounds.size.height)
                            };
                            if length > px(0.)
                                && let Some(layout) = &mut ws.layout
                            {
                                layout.resize(&address, offset / length);
                                cx.notify();
                            }
                        },
                    ))
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, Layout, Pane};
    #[test]
    fn nested_layout_preserves_siblings_and_recovers_hidden_panes() {
        let mut p = Layout::new("a".into());
        p.split("a", "b", Direction::Across);
        p.split("b", "c", Direction::Down);
        p.resize("R", 1.5);
        assert_eq!(p.ids(), ["a", "b", "c"]);
        assert!(
            matches!(&p.root, Pane::Split { second, .. } if matches!(**second, Pane::Split {fraction:0.70,..}))
        );
        assert_eq!(p.minimize("a").as_deref(), Some("b"));
        p.zoom("c");
        assert!(!p.visible("b") && p.visible("c"));
        p.open("c", "a");
        assert!(p.visible("a") && p.visible("b"));
        p.open("a", "d");
        assert_eq!(p.ids(), ["d", "b", "c"]);
        p.root = p.root.remove("b").unwrap();
        assert_eq!(p.ids(), ["d", "c"]);
    }
}
