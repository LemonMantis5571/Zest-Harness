//! Customize > Advisor: the stronger model a chat can consult (`[advisor]` in `zest.toml`).
//! Choose its provider (one whose agent loop Zest drives: an API key or ChatGPT sign-in) and
//! model, how many consultations a request may make, and whether Zest may consult on its own.
//! `config_edit::set_advisor` validates the selection before it writes.

use super::*;
use crate::theme::Icon;
use crate::ui::{self, Variant};
use zest_core::config_edit::{self, AdvisorInput};

/// The choices on the page before they are saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AdvisorDraft {
    provider: Option<String>,
    model: Option<String>,
    max_uses: u32,
    auto_review: bool,
}

impl Default for AdvisorDraft {
    fn default() -> Self {
        Self { provider: None, model: None, max_uses: 3, auto_review: true }
    }
}

/// Consultation limits offered as buttons; `config_edit` accepts 1 to 20.
const USES: [u32; 5] = [1, 2, 3, 5, 10];

/// Providers that can serve as the advisor: a vendor CLI runs its own tools and cannot simply
/// answer a question, so only providers on Zest's own loop qualify.
pub(super) fn advisor_providers(config: &zest_core::Config) -> Vec<String> {
    config
        .providers
        .iter()
        .filter(|(_, provider)| !provider.owns_agent_loop())
        .map(|(id, _)| id.clone())
        .collect()
}

impl Workspace {
    /// Starts the page from the saved `[advisor]`, or the defaults when there is none.
    pub(super) fn load_advisor(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.project.reload_config() {
            self.toast(ToastKind::Error, e.to_string(), cx);
        }
        self.advisor_draft = match &self.project.config.advisor {
            Some(saved) => AdvisorDraft {
                provider: Some(saved.provider.clone()),
                model: saved.model.clone(),
                max_uses: saved.max_uses,
                auto_review: saved.auto_review,
            },
            None => AdvisorDraft::default(),
        };
        cx.notify();
    }

    fn save_advisor(&mut self, off: bool, cx: &mut Context<Self>) {
        let draft = self.advisor_draft.clone();
        let input = match (&draft.provider, off) {
            (_, true) => None,
            (Some(provider), false) => Some(AdvisorInput {
                provider: provider.clone(),
                model: draft.model.clone(),
                max_uses: draft.max_uses,
                auto_review: draft.auto_review,
            }),
            (None, false) => {
                self.toast(ToastKind::Error, "Choose the advisor's provider first.", cx);
                return;
            }
        };
        let result = zest_core::mcp_admin::editable_config_path(&self.project.config_root)
            .and_then(|path| config_edit::set_advisor(&path, input.as_ref()));
        match result {
            Ok(()) => {
                self.load_advisor(cx);
                let message = match input {
                    None => "The advisor is off.".to_string(),
                    Some(input) => format!("New chats consult {} as the advisor.", input.model.as_deref().unwrap_or(&input.provider)),
                };
                self.toast(ToastKind::Success, message, cx);
            }
            Err(e) => self.toast(ToastKind::Error, e, cx),
        }
    }

    pub(super) fn advisor_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let draft = &self.advisor_draft;
        let config = &self.project.config;
        let mut body = div().flex().flex_col().gap_5();
        if self.project.demo {
            return body
                .child(ui::empty_state(
                    Icon::Users,
                    "Advisor unavailable in offline demo",
                    "Open a project window to configure a second-opinion advisor model.",
                ))
                .into_any_element();
        }
        let saved = config.advisor.as_ref();
        body = body
            .child(ui::section_heading(
                "Advisor",
                "A stronger model your chat can consult for a second opinion before finishing or after failures.",
            ))
            .child(
                div()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme::alpha(theme::border(), 0.7))
                    .bg(theme::alpha(theme::card(), 0.55))
                    .px_3()
                    .py(px(10.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(theme::muted_foreground()))
                    .child(match saved {
                        Some(advisor) => format!(
                            "Active: {} · {}. Each request may consult it up to {} time{}.",
                            advisor.provider,
                            advisor.model.as_deref().unwrap_or("default model"),
                            advisor.max_uses,
                            if advisor.max_uses == 1 { "" } else { "s" }
                        ),
                        None => "Off. Choose a provider and model below, then save to turn it on.".to_string(),
                    }),
            );
        let providers = advisor_providers(config);
        if providers.is_empty() {
            return body
                .child(ui::empty_state(
                    Icon::Sparkles,
                    "No eligible advisor providers yet",
                    "The advisor needs a provider on Zest's own loop: an API key or ChatGPT sign-in. Add one in Customize > Providers.",
                ))
                .into_any_element();
        }
        let mut provider_list = div().flex().flex_col().gap_2();
        for id in providers {
            let selected = draft.provider.as_deref() == Some(id.as_str());
            let pick = id.clone();
            let title = config
                .providers
                .get(&id)
                .map(|cfg| provider_display_name(&id, cfg))
                .unwrap_or_else(|| id.clone());
            let detail = config
                .providers
                .get(&id)
                .map(|cfg| {
                    let d = zest_core::descriptor_from_config(&id, cfg);
                    format!("{} · {} models", id, d.models.len().max(1))
                })
                .unwrap_or_else(|| id.clone());
            provider_list = provider_list.child(
                div()
                    .id(SharedString::from(format!("advisor-provider-{id}")))
                    .role(Role::RadioButton)
                    .aria_label(SharedString::from(if selected {
                        format!("{title}, current")
                    } else {
                        title.clone()
                    }))
                    .aria_toggled(if selected { accesskit::Toggled::True } else { accesskit::Toggled::False })
                    .tab_index(0)
                    .cursor_pointer()
                    .px_3()
                    .py(px(9.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(if selected {
                        theme::alpha(theme::primary(), 0.8)
                    } else {
                        theme::alpha(theme::border(), 0.8)
                    })
                    .bg(if selected {
                        theme::alpha(theme::primary(), 0.12)
                    } else {
                        theme::alpha(theme::card(), 0.7)
                    })
                    .hover(|s| {
                        if selected {
                            s.bg(theme::alpha(theme::primary(), 0.16))
                        } else {
                            s.bg(rgb(theme::card())).border_color(rgb(theme::border()))
                        }
                    })
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded(px(7.))
                            .bg(rgb(theme::secondary()))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(theme::provider_icon(&id, None, 14.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(title))
                            .child(div().truncate().text_size(px(11.)).text_color(rgb(theme::muted_foreground())).child(detail)),
                    )
                    .when(selected, |d| d.child(ui::check_badge()))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.advisor_draft.provider = Some(pick.clone());
                        ws.advisor_draft.model = None;
                        cx.notify();
                    })),
            );
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(rgb(theme::muted_foreground())).child("Provider"))
                .child(provider_list),
        );
        if let Some(provider) = draft.provider.as_deref().and_then(|id| config.providers.get(id).map(|cfg| (id, cfg))) {
            let descriptor = zest_core::descriptor_from_config(provider.0, provider.1);
            let mut models = div().flex().flex_wrap().gap_2();
            let default_selected = draft.model.is_none();
            models = models.child(ui::select_chip(
                "advisor-model-default",
                format!("Default ({})", descriptor.default_model),
                default_selected,
                cx.listener(|ws, _, _, cx| {
                    ws.advisor_draft.model = None;
                    cx.notify();
                }),
            ));
            for spec in descriptor.models.iter().take(12) {
                let selected = draft.model.as_deref() == Some(spec.id.as_str());
                let pick = spec.id.clone();
                models = models.child(ui::select_chip(
                    SharedString::from(format!("advisor-model-{}", spec.id)),
                    spec.id.clone(),
                    selected,
                    cx.listener(move |ws, _, _, cx| {
                        ws.advisor_draft.model = Some(pick.clone());
                        cx.notify();
                    }),
                ));
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(rgb(theme::muted_foreground())).child("Model"))
                    .child(models),
            );
        }
        let mut uses = div().flex().flex_wrap().gap_2();
        for n in USES {
            let selected = draft.max_uses == n;
            uses = uses.child(ui::select_chip(
                SharedString::from(format!("advisor-uses-{n}")),
                n.to_string(),
                selected,
                cx.listener(move |ws, _, _, cx| {
                    ws.advisor_draft.max_uses = n;
                    cx.notify();
                }),
            ));
        }
        body = body
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(rgb(theme::muted_foreground())).child("Consultations per request"))
                    .child(uses),
            )
            .child(ui::switch_row(
                "advisor-auto",
                "Review automatically",
                "Zest also asks the advisor itself: before a turn that changed files ends, and after two failed tool calls in a row.",
                draft.auto_review,
                cx.listener(|ws, on: &bool, _, cx| {
                    ws.advisor_draft.auto_review = *on;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .pt_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::button("advisor-save", Variant::Primary, None, "Save advisor", cx.listener(|ws, _, _, cx| ws.save_advisor(false, cx))))
                    .when(saved.is_some(), |d| {
                        d.child(ui::button("advisor-off", Variant::Outline, None, "Turn off", cx.listener(|ws, _, _, cx| ws.save_advisor(true, cx))))
                    }),
            );
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::advisor_providers;

    #[::core::prelude::v1::test]
    fn only_providers_on_zests_loop_can_advise() {
        let config = zest_core::Config::parse(
            "[providers.claude]\nkind = \"claude_code\"\n[providers.key]\nkind = \"anthropic\"\n[providers.gpt]\nkind = \"codex_oauth\"\n",
        )
        .unwrap();
        assert_eq!(advisor_providers(&config), ["gpt", "key"]);
    }
}
