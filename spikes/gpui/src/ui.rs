//! Reusable controls styled like the React UI (shadcn/Tailwind on the "zest" theme).

use crate::theme::{self, Icon, alpha, icon};
use gpui::prelude::*;
use gpui::*;

/// Sidebar navigation row: icon, label and optional trailing text.
pub fn nav_item(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    selected: bool,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .h(px(28.))
        .px_2()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(rgb(theme::FOREGROUND))
        .when(selected, |d| d.bg(rgb(theme::SECONDARY)))
        .hover(|s| s.bg(rgb(theme::SECONDARY)))
        .focus_visible(|s| s.bg(rgb(theme::ACCENT)))
        .child(icon(glyph, 16.).text_color(rgb(theme::MUTED_FOREGROUND)))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .on_click(click)
}

/// Square ghost button holding one icon (shadcn `icon-sm`: 28 px, 16 px glyph).
/// `label` is its accessible name.
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    sized_icon_button(id, glyph, label, 28., 16., click)
}

/// Smaller icon button (shadcn `icon-xs`: 24 px, 12 px glyph) for row actions.
pub fn icon_button_xs(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    sized_icon_button(id, glyph, label, 24., 12., click)
}

fn sized_icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    side: f32,
    glyph_size: f32,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let group = SharedString::from(format!("icon-button-{id:?}"));
    div()
        .id(id)
        .group(group.clone())
        .role(Role::Button)
        .aria_label(label.into())
        .tab_index(0)
        .cursor_pointer()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(rgb(theme::MUTED_FOREGROUND))
        .hover(|s| s.bg(rgb(theme::SECONDARY)).text_color(rgb(theme::FOREGROUND)))
        .focus_visible(|s| s.bg(rgb(theme::ACCENT)).text_color(rgb(theme::FOREGROUND)))
        .child(
            icon(glyph, glyph_size)
                .group_hover(group, |s| s.text_color(rgb(theme::FOREGROUND))),
        )
        .on_click(click)
}

/// Uppercase section heading such as "PROJECTS".
pub fn section_label(glyph: Icon, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(theme::MUTED_FOREGROUND))
        .child(icon(glyph, 14.))
        .child(label.to_uppercase())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Outline,
    Ghost,
    Destructive,
}

/// Text button with an optional leading icon.
pub fn button(
    id: impl Into<ElementId>,
    variant: Variant,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    let base = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .h(px(28.))
        .px(px(10.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .rounded(px(6.))
        .text_size(px(12.8))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap();
    let styled = match variant {
        Variant::Primary => base
            .bg(rgb(theme::PRIMARY))
            .text_color(rgb(0xffffff))
            .hover(|s| s.bg(alpha(theme::PRIMARY, 0.8))),
        Variant::Secondary => base
            .bg(rgb(theme::SECONDARY))
            .text_color(rgb(theme::SECONDARY_FOREGROUND))
            .hover(|s| s.bg(rgb(theme::ACCENT))),
        Variant::Outline => base
            .border_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(0x17181b))
            .text_color(rgb(theme::FOREGROUND))
            .hover(|s| s.bg(rgb(0x1f2024))),
        Variant::Ghost => base
            .text_color(rgb(theme::SECONDARY_FOREGROUND))
            .hover(|s| s.bg(rgb(theme::SECONDARY)).text_color(rgb(theme::FOREGROUND))),
        Variant::Destructive => base
            .bg(alpha(theme::DESTRUCTIVE, 0.2))
            .text_color(rgb(theme::DESTRUCTIVE))
            .hover(|s| s.bg(alpha(theme::DESTRUCTIVE, 0.3))),
    };
    styled
        .focus_visible(|s| s.border_1().border_color(rgb(theme::PRIMARY)))
        .when_some(glyph, |d, g| {
            let color = match variant {
                Variant::Primary => 0xffffff,
                Variant::Destructive => theme::DESTRUCTIVE,
                Variant::Outline => theme::FOREGROUND,
                Variant::Secondary | Variant::Ghost => theme::SECONDARY_FOREGROUND,
            };
            d.child(icon(g, 14.).text_color(rgb(color)))
        })
        .child(label)
        .on_click(click)
}

/// Bordered surface used for panels, cards and the composer.
pub fn card() -> Div {
    div()
        .bg(rgb(theme::CARD))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .rounded(px(12.))
}

/// Small rounded label, e.g. a pull-request number or a status.
pub fn pill(glyph: Option<Icon>, label: impl Into<SharedString>) -> Div {
    div()
        .h(px(22.))
        .px(px(6.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme::BORDER))
        .text_size(px(12.))
        .text_color(rgb(theme::SECONDARY_FOREGROUND))
        .when_some(glyph, |d, g| d.child(icon(g, 12.)))
        .child(label.into())
}


/// Heading of a group of settings, like the React panels' `h2` (13 px semibold).
pub fn section_title(text: impl Into<SharedString>) -> Div {
    div()
        .mt_2()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::FOREGROUND))
        .child(text.into())
}

/// Muted helper or status line under a section.
pub fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(theme::MUTED_FOREGROUND))
        .child(text.into())
}
