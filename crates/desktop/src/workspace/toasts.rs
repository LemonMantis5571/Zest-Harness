//! Toasts: short notices stacked in the window's bottom-right corner. Errors stay until
//! dismissed, because an error is something to read, not to race; confirmations leave after
//! 3 s and other notices after 5 s. A repeat of a toast still on screen counts on it ("×2")
//! instead of stacking a copy, and restarts its timer.

use super::*;
use crate::theme::{self, Icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastKind {
    /// How long a toast of this kind stays; `None` keeps it until dismissed.
    pub(crate) fn lifetime(self) -> Option<Duration> {
        match self {
            ToastKind::Error => None,
            ToastKind::Success => Some(Duration::from_secs(3)),
            ToastKind::Info | ToastKind::Warning => Some(Duration::from_secs(5)),
        }
    }

    fn icon(self) -> (Icon, u32) {
        match self {
            ToastKind::Info => (Icon::Bell, theme::muted_foreground()),
            ToastKind::Success => (Icon::CircleCheck, theme::primary()),
            ToastKind::Warning => (Icon::TriangleAlert, theme::WARNING),
            ToastKind::Error => (Icon::CircleX, theme::destructive()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            ToastKind::Info => "Notice",
            ToastKind::Success => "Done",
            ToastKind::Warning => "Warning",
            ToastKind::Error => "Error",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Toast {
    pub(crate) id: u64,
    pub(crate) kind: ToastKind,
    pub(crate) text: String,
    pub(crate) count: u32,
    /// Bumped on every repeat, so an older timer does not remove a toast that was just renewed.
    stamp: u64,
}

/// At most this many toasts are kept; the oldest goes first.
const MAX_TOASTS: usize = 5;

#[derive(Default)]
pub(crate) struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    /// Shows `text`, or counts a repeat of the same toast. Returns its id and stamp for the
    /// expiry timer.
    pub(crate) fn push(&mut self, kind: ToastKind, text: String) -> (u64, u64) {
        self.next += 1;
        let stamp = self.next;
        if let Some(existing) = self.items.iter_mut().find(|t| t.kind == kind && t.text == text) {
            existing.count += 1;
            existing.stamp = stamp;
            return (existing.id, stamp);
        }
        self.items.push(Toast { id: stamp, kind, text, count: 1, stamp });
        if self.items.len() > MAX_TOASTS {
            self.items.remove(0);
        }
        (stamp, stamp)
    }

    /// Removes the toast if no repeat renewed it since `stamp`. Returns whether it went.
    pub(crate) fn expire(&mut self, id: u64, stamp: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|t| t.id != id || t.stamp != stamp);
        self.items.len() != before
    }

    pub(crate) fn dismiss(&mut self, id: u64) {
        self.items.retain(|t| t.id != id);
    }

    pub(crate) fn items(&self) -> &[Toast] {
        &self.items
    }
}

impl Workspace {
    /// Shows a toast. Blank text is ignored, so `Err(e) => ws.toast(Error, e)` needs no check.
    pub(crate) fn toast(&mut self, kind: ToastKind, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        let (id, stamp) = self.toasts.push(kind, text);
        if let Some(lifetime) = kind.lifetime() {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(lifetime).await;
                let _ = this.update(cx, |ws, cx| {
                    if ws.toasts.expire(id, stamp) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Shows a toast that stays until dismissed, for instructions the user acts on elsewhere
    /// (finishing a sign-in in the browser).
    pub(crate) fn toast_pinned(&mut self, kind: ToastKind, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if !text.trim().is_empty() {
            self.toasts.push(kind, text);
            cx.notify();
        }
    }

    /// The corner stack: attention alerts ("Open · …") first, then toasts, newest last.
    pub(super) fn toast_stack(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.toasts.items().is_empty() && self.alerts.is_empty() {
            return None;
        }
        let card = || {
            div()
                .w(px(360.))
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(theme::border()))
                .bg(rgb(theme::card()))
                .shadow_md()
                .text_size(px(13.))
        };
        let alerts = self.alerts.iter().enumerate().map(|(i, (id, _, label))| {
            let target = id.clone();
            let dismissed = id.clone();
            card()
                .id(SharedString::from(format!("alert-{i}")))
                .role(Role::Alert)
                .aria_label(SharedString::from(format!("Needs attention: {label}")))
                .child(theme::icon(Icon::Bell, 14.).text_color(rgb(theme::WARNING)))
                .child(div().flex_1().min_w_0().child(label.clone()))
                .child(crate::ui::button(
                    SharedString::from(format!("alert-open-{i}")),
                    crate::ui::Variant::Outline,
                    None,
                    "Open",
                    cx.listener(move |ws, _, w, cx| ws.open(&target, w, cx)),
                ))
                .child(crate::ui::icon_button_xs(
                    SharedString::from(format!("alert-dismiss-{i}")),
                    Icon::X,
                    "Dismiss alert",
                    cx.listener(move |ws, _, _, cx| {
                        ws.alerts.retain(|(owner, _, _)| owner != &dismissed);
                        cx.notify();
                    }),
                ))
                .into_any_element()
        });
        let toasts = self.toasts.items().iter().map(|t| {
            let (glyph, color) = t.kind.icon();
            let id = t.id;
            let text = if t.count > 1 { format!("{} ×{}", t.text, t.count) } else { t.text.clone() };
            card()
                .id(SharedString::from(format!("toast-{id}")))
                .role(if t.kind == ToastKind::Error { Role::Alert } else { Role::Status })
                .aria_label(SharedString::from(format!("{}: {text}", t.kind.label())))
                .child(theme::icon(glyph, 14.).text_color(rgb(color)))
                .child(div().flex_1().min_w_0().child(text))
                .child(crate::ui::icon_button_xs(
                    SharedString::from(format!("toast-dismiss-{id}")),
                    Icon::X,
                    "Dismiss",
                    cx.listener(move |ws, _, _, cx| {
                        ws.toasts.dismiss(id);
                        cx.notify();
                    }),
                ))
                .into_any_element()
        });
        Some(
            div()
                .absolute()
                .right(px(16.))
                .bottom(px(16.))
                .flex()
                .flex_col()
                .items_end()
                .gap_2()
                .children(alerts)
                .children(toasts)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ToastKind, Toasts};
    use std::time::Duration;

    #[::core::prelude::v1::test]
    fn errors_stay_and_notices_leave() {
        assert_eq!(ToastKind::Error.lifetime(), None);
        assert_eq!(ToastKind::Success.lifetime(), Some(Duration::from_secs(3)));
        assert_eq!(ToastKind::Info.lifetime(), Some(Duration::from_secs(5)));
        assert_eq!(ToastKind::Warning.lifetime(), Some(Duration::from_secs(5)));
    }

    #[::core::prelude::v1::test]
    fn a_repeat_counts_on_the_toast_and_renews_its_timer() {
        let mut toasts = Toasts::default();
        let (id, first) = toasts.push(ToastKind::Error, "Could not save".into());
        let (same, second) = toasts.push(ToastKind::Error, "Could not save".into());
        assert_eq!(id, same);
        assert_eq!(toasts.items().len(), 1);
        assert_eq!(toasts.items()[0].count, 2);
        assert!(!toasts.expire(id, first), "the first timer must not remove a renewed toast");
        assert!(toasts.expire(id, second));
        assert!(toasts.items().is_empty());
    }

    #[::core::prelude::v1::test]
    fn different_text_or_kind_stacks_and_the_oldest_goes_first() {
        let mut toasts = Toasts::default();
        toasts.push(ToastKind::Info, "Signed in.".into());
        toasts.push(ToastKind::Success, "Signed in.".into());
        for n in 0..5 {
            toasts.push(ToastKind::Error, format!("failure {n}"));
        }
        let texts: Vec<_> = toasts.items().iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["failure 0", "failure 1", "failure 2", "failure 3", "failure 4"]);
    }

    #[::core::prelude::v1::test]
    fn dismiss_removes_only_that_toast() {
        let mut toasts = Toasts::default();
        let (a, _) = toasts.push(ToastKind::Error, "a".into());
        toasts.push(ToastKind::Error, "b".into());
        toasts.dismiss(a);
        assert_eq!(toasts.items().len(), 1);
        assert_eq!(toasts.items()[0].text, "b");
    }
}
