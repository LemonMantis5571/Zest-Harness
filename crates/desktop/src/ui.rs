//! Reusable controls: buttons, cards, pills, switches and section headings.

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
        .text_color(rgb(theme::foreground()))
        .when(selected, |d| d.bg(rgb(theme::secondary())))
        .hover(|s| s.bg(rgb(theme::secondary())))
        .focus_visible(|s| s.bg(rgb(theme::accent())))
        .child(icon(glyph, 16.).text_color(rgb(theme::muted_foreground())))
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
        .text_color(rgb(theme::muted_foreground()))
        .hover(|s| s.bg(rgb(theme::secondary())).text_color(rgb(theme::foreground())))
        .focus_visible(|s| s.bg(rgb(theme::accent())).text_color(rgb(theme::foreground())))
        .child(
            icon(glyph, glyph_size)
                .group_hover(group, |s| s.text_color(rgb(theme::foreground()))),
        )
        .on_click(click)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Outline,
    Ghost,
    Destructive,
}

/// Text button with an optional leading icon. Passing `Icon::LoaderCircle` animates it automatically.
pub fn button(
    id: impl Into<ElementId>,
    variant: Variant,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let label = label.into();
    let base = div()
        .id(id.clone())
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
            .bg(rgb(theme::primary()))
            .text_color(rgb(crate::theme::ON_PRIMARY))
            .hover(|s| s.bg(alpha(theme::primary(), 0.8))),
        Variant::Secondary => base
            .bg(rgb(theme::secondary()))
            .text_color(rgb(theme::secondary_foreground()))
            .hover(|s| s.bg(rgb(theme::accent()))),
        Variant::Outline => base
            .border_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(crate::theme::input_bg()))
            .text_color(rgb(theme::foreground()))
            .hover(|s| s.bg(rgb(crate::theme::input_hover()))),
        Variant::Ghost => base
            .text_color(rgb(theme::secondary_foreground()))
            .hover(|s| s.bg(rgb(theme::secondary())).text_color(rgb(theme::foreground()))),
        Variant::Destructive => base
            .bg(alpha(theme::destructive(), 0.2))
            .text_color(rgb(theme::destructive()))
            .hover(|s| s.bg(alpha(theme::destructive(), 0.3))),
    };
    styled
        .focus_visible(|s| s.border_1().border_color(rgb(theme::primary())))
        .when_some(glyph, |d, g| {
            let color = match variant {
                Variant::Primary => crate::theme::ON_PRIMARY,
                Variant::Destructive => theme::destructive(),
                Variant::Outline => theme::foreground(),
                Variant::Secondary | Variant::Ghost => theme::secondary_foreground(),
            };
            if g == Icon::LoaderCircle {
                d.child(theme::spinner_colored(
                    SharedString::from(format!("btn-spin-{id:?}")),
                    14.,
                    color,
                ))
            } else {
                d.child(icon(g, 14.).text_color(rgb(color)))
            }
        })
        .child(label)
        .on_click(click)
}

/// Bordered surface used for panels, cards and the composer.
pub fn card() -> Div {
    div()
        .bg(rgb(theme::card()))
        .border_1()
        .border_color(rgb(theme::border()))
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
        .border_color(rgb(theme::border()))
        .text_size(px(12.))
        .text_color(rgb(theme::secondary_foreground()))
        .when_some(glyph, |d, g| d.child(icon(g, 12.)))
        .child(label.into())
}


/// Heading of a group of settings (13 px semibold).
pub fn section_title(text: impl Into<SharedString>) -> Div {
    div()
        .mt_2()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::foreground()))
        .child(text.into())
}

/// Section title + muted description matching pre-GPUI `SectionHeading`.
pub fn section_heading(title: impl Into<SharedString>, hint: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::foreground()))
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(11.5))
                .line_height(px(17.))
                .text_color(rgb(theme::muted_foreground()))
                .child(hint.into()),
        )
}

/// Bordered chip button for model, effort, filter, or option selection.
pub fn select_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_toggled(if selected { accesskit::Toggled::True } else { accesskit::Toggled::False })
        .tab_index(0)
        .cursor_pointer()
        .h(px(30.))
        .px(px(11.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(if selected {
            alpha(theme::primary(), 0.8)
        } else {
            alpha(theme::border(), 0.85)
        })
        .bg(if selected {
            alpha(theme::primary(), 0.14)
        } else {
            alpha(theme::card(), 0.75)
        })
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(if selected {
            theme::foreground()
        } else {
            theme::secondary_foreground()
        }))
        .hover(|s| {
            if selected {
                s.bg(alpha(theme::primary(), 0.2))
            } else {
                s.bg(rgb(theme::secondary())).text_color(rgb(theme::foreground()))
            }
        })
        .focus_visible(|s| s.border_color(rgb(theme::primary())))
        .when(selected, |d| {
            d.child(icon(Icon::Check, 12.).text_color(rgb(theme::primary())))
        })
        .child(label)
        .on_click(click)
}

/// Centered empty-state card with icon badge, heading, and guidance copy.
pub fn empty_state(
    glyph: Icon,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    div()
        .min_h(px(220.))
        .rounded(px(12.))
        .border_1()
        .border_color(alpha(theme::border(), 0.65))
        .bg(alpha(theme::card(), 0.45))
        .px_6()
        .py(px(40.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .mb_2()
                .p_3()
                .rounded(px(12.))
                .border_1()
                .border_color(alpha(theme::border(), 0.65))
                .bg(alpha(theme::secondary(), 0.6))
                .child(icon(glyph, 20.).text_color(rgb(theme::muted_foreground()))),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(theme::foreground()))
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(360.))
                .text_center()
                .text_size(px(12.5))
                .line_height(px(19.))
                .text_color(rgb(theme::muted_foreground()))
                .child(detail.into()),
        )
}

/// Circular primary checkmark badge used on selected radio/theme/font/provider cards.
pub fn check_badge() -> Div {
    div()
        .size(px(16.))
        .flex_none()
        .rounded_full()
        .bg(rgb(theme::primary()))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(Icon::Check, 10.).text_color(rgb(theme::ON_PRIMARY)))
}

/// Muted helper or status line under a section.
pub fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(theme::muted_foreground()))
        .child(text.into())
}

/// Muted status line with a spinning loader icon.
pub fn loading_row(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(12.))
        .text_color(rgb(theme::muted_foreground()))
        .child(theme::spinner(id, 13.))
        .child(text.into())
}

/// On/off switch following UI SDK's switch behavior: the switch role with a toggled state,
/// toggled by click, Enter or Space. 36 x 20 px.
pub fn switch(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Switch)
        .aria_label(label.into())
        .aria_toggled(if checked { accesskit::Toggled::True } else { accesskit::Toggled::False })
        .tab_index(0)
        .cursor_pointer()
        .w(px(36.))
        .h(px(20.))
        .flex_none()
        .relative()
        .rounded_full()
        .bg(rgb(if checked { theme::primary() } else { theme::secondary() }))
        .border_1()
        .border_color(rgb(if checked { theme::primary() } else { theme::border() }))
        .focus_visible(|s| s.border_color(rgb(theme::primary_hover())))
        .child(
            div()
                .absolute()
                .top(px(1.))
                .left(px(if checked { 17. } else { 1. }))
                .size(px(16.))
                .rounded_full()
                .bg(rgb(if checked { theme::ON_PRIMARY } else { theme::muted_foreground() })),
        )
        .on_click(move |_, window, cx| toggle(&!checked, window, cx))
}

/// A labelled row holding a switch, for settings pages.
pub fn switch_row(
    id: impl Into<ElementId>,
    title: &'static str,
    hint: &'static str,
    checked: bool,
    toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Div {
    card()
        .rounded(px(8.))
        .px_3()
        .py(px(10.))
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(title))
                .child(hint_text(hint)),
        )
        .child(switch(id, title, checked, toggle))
}

fn hint_text(text: &'static str) -> Div {
    div().text_size(px(11.)).text_color(rgb(theme::muted_foreground())).child(text)
}
