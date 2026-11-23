//! Customize > Advisor: the stronger model a chat can consult (`[advisor]` in `zest.toml`).
//! Choose its provider (one whose agent loop Zest drives: an API key or ChatGPT sign-in) and
//! model, how many consultations a request may make, and whether Zest may consult on its own.
//! `config_edit::set_advisor` validates the selection before it writes.

use super::*;
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
        let mut body = div().flex().flex_col().gap_3();
        if self.project.demo {
            return body.child(ui::hint("The offline demo has no advisor. Set one up in a project window.")).into_any_element();
        }
        let saved = config.advisor.as_ref();
        body = body.child(ui::hint(match saved {
            Some(advisor) => format!(
                "On: {} · {}. The agent can ask it for a second opinion; each request may consult it up to {} time{}.",
                advisor.provider,
                advisor.model.as_deref().unwrap_or("default model"),
                advisor.max_uses,
                if advisor.max_uses == 1 { "" } else { "s" }
            ),
            None => "Off. An advisor is a stronger model the agent can consult before finishing or after failures.".to_string(),
        }));
        let providers = advisor_providers(config);
        if providers.is_empty() {
            return body
                .child(ui::hint("The advisor needs a provider on Zest's own loop: an API key or ChatGPT sign-in. Add one in Customize > Providers."))
                .into_any_element();
        }
        let mut row = div().flex().flex_wrap().gap_1();
        for id in providers {
            let selected = draft.provider.as_deref() == Some(id.as_str());
            let pick = id.clone();
            row = row.child(ui::button(
                SharedString::from(format!("advisor-provider-{id}")),
                if selected { Variant::Secondary } else { Variant::Ghost },
                None,
                if selected { format!("{id}, current") } else { id.clone() },
                cx.listener(move |ws, _, _, cx| {
                    ws.advisor_draft.provider = Some(pick.clone());
                    ws.advisor_draft.model = None;
                    cx.notify();
                }),
            ));
        }
        body = body.child(ui::section_title("Provider")).child(row);
        if let Some(provider) = draft.provider.as_deref().and_then(|id| config.providers.get(id).map(|cfg| (id, cfg))) {
            let descriptor = zest_core::descriptor_from_config(provider.0, provider.1);
            let mut models = div().flex().flex_wrap().gap_1();
            let default_selected = draft.model.is_none();
            models = models.child(ui::button(
                "advisor-model-default",
                if default_selected { Variant::Secondary } else { Variant::Ghost },
                None,
                format!("Default ({})", descriptor.default_model),
                cx.listener(|ws, _, _, cx| {
                    ws.advisor_draft.model = None;
                    cx.notify();
                }),
            ));
            for spec in descriptor.models.iter().take(12) {
                let selected = draft.model.as_deref() == Some(spec.id.as_str());
                let pick = spec.id.clone();
                models = models.child(ui::button(
                    SharedString::from(format!("advisor-model-{}", spec.id)),
                    if selected { Variant::Secondary } else { Variant::Ghost },
                    None,
                    spec.id.clone(),
                    cx.listener(move |ws, _, _, cx| {
                        ws.advisor_draft.model = Some(pick.clone());
                        cx.notify();
                    }),
                ));
            }
            body = body.child(ui::section_title("Model")).child(models);
        }
        let mut uses = div().flex().gap_1();
        for n in USES {
            let selected = draft.max_uses == n;
            uses = uses.child(ui::button(
                SharedString::from(format!("advisor-uses-{n}")),
                if selected { Variant::Secondary } else { Variant::Ghost },
                None,
                n.to_string(),
                cx.listener(move |ws, _, _, cx| {
                    ws.advisor_draft.max_uses = n;
                    cx.notify();
                }),
            ));
        }
        body = body
            .child(ui::section_title("Consultations per request"))
            .child(uses)
            .child(ui::switch_row(
                "advisor-auto",
                "Consult on its own",
                "Before a turn that changed files finishes, and after two failed tool calls in a row, when that needs no new approval.",
                draft.auto_review,
                cx.listener(|ws, on: &bool, _, cx| {
                    ws.advisor_draft.auto_review = *on;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(ui::button("advisor-save", Variant::Primary, None, "Save advisor", cx.listener(|ws, _, _, cx| ws.save_advisor(false, cx))))
                    .when(saved.is_some(), |d| {
                        d.child(ui::button("advisor-off", Variant::Ghost, None, "Turn off", cx.listener(|ws, _, _, cx| ws.save_advisor(true, cx))))
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
