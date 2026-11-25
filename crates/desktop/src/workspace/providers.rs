//! Customize > Providers: the chat providers this project can use. Add one (a vendor CLI or
//! ChatGPT sign-in in one click, an API provider with its key), sign in, save an API key, and
//! choose the provider new chats use. A project with no provider opens on this page. The work
//! happens in `zest_core::provider_setup`.

use super::*;
use crate::composer::Composer;
use crate::theme::{self, Icon};
use crate::ui::{self, Variant};
use zest_core::provider_setup::{self, ProviderChoice, ProviderForm, ProviderRow, ProviderState};

/// Fields for adding an API provider, or for saving a key for a configured one.
pub(super) struct KeyForm {
    /// The provider kind being added; `None` when saving a key for `id`.
    adding: Option<ProviderChoice>,
    id: String,
    key: Entity<Composer>,
    model: Entity<Composer>,
    base_url: Entity<Composer>,
}

#[derive(Default)]
pub(super) struct ProvidersPage {
    rows: Vec<ProviderRow>,
    form: Option<KeyForm>,
}

fn field(cx: &mut Context<Workspace>, placeholder: &'static str) -> Entity<Composer> {
    cx.new(|cx| {
        let mut field = Composer::new(cx);
        field.placeholder = placeholder;
        field
    })
}

fn signs_in(choice: Option<ProviderChoice>) -> bool {
    matches!(choice, Some(ProviderChoice::ClaudeCode | ProviderChoice::CodexCli | ProviderChoice::ChatGpt))
}

impl Workspace {
    /// Reads the providers again and reloads the project config so new chats see the change.
    pub(super) fn load_providers(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.project.reload_config() {
            self.toast(ToastKind::Error, e.to_string(), cx);
        }
        self.providers_page.rows = provider_setup::rows(&self.project.config);
        cx.notify();
    }

    fn add_provider(&mut self, choice: ProviderChoice, form: ProviderForm, cx: &mut Context<Self>) {
        match provider_setup::add(&self.project.config_root, choice, form) {
            Ok(id) => {
                self.providers_page.form = None;
                self.load_providers(cx);
                let ready = self.providers_page.rows.iter().any(|r| r.id == id && r.state == ProviderState::Ready);
                let next = if ready || choice.needs_key() { "New chats can use it." } else { "Sign in to use it." };
                self.toast(ToastKind::Success, format!("Added {id}. {next}"), cx);
            }
            Err(e) => self.toast(ToastKind::Error, e, cx),
        }
    }

    fn open_key_form(&mut self, adding: Option<ProviderChoice>, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let form = KeyForm {
            adding,
            id,
            key: field(cx, "API key"),
            model: field(cx, if adding == Some(ProviderChoice::OpenAiCompatible) { "Model, for example gpt-6.2-sol" } else { "Model (blank for the default)" }),
            base_url: field(cx, "Endpoint, for example https://api.openai.com/v1"),
        };
        window.focus(&form.key.read(cx).focus.clone(), cx);
        self.providers_page.form = Some(form);
        cx.notify();
    }

    fn submit_key_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.providers_page.form.as_ref() else { return };
        let read = |f: &Entity<Composer>| f.read(cx).text.trim().to_string();
        let (key, model, base_url) = (read(&form.key), read(&form.model), read(&form.base_url));
        match form.adding {
            Some(choice) => {
                let request = ProviderForm { id: String::new(), model, base_url, key };
                self.add_provider(choice, request, cx);
            }
            None => {
                let id = form.id.clone();
                match provider_setup::save_key(&self.project.config, &id, &key) {
                    Ok(()) => {
                        self.providers_page.form = None;
                        self.load_providers(cx);
                        self.toast(ToastKind::Success, format!("Saved the API key for {id}."), cx);
                    }
                    Err(e) => self.toast(ToastKind::Error, e, cx),
                }
            }
        }
    }

    fn use_for_new_chats(&mut self, id: String, cx: &mut Context<Self>) {
        match provider_setup::make_default(&self.project.config_root, &id) {
            Ok(()) => {
                self.load_providers(cx);
                self.toast(ToastKind::Success, format!("New chats use {id}."), cx);
            }
            Err(e) => self.toast(ToastKind::Error, e, cx),
        }
    }

    pub(super) fn providers_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.providers_page;
        let mut body = div().flex().flex_col().gap_5();
        if self.project.demo {
            return body
                .child(ui::empty_state(
                    Icon::Sparkles,
                    "Offline demo provider active",
                    "Open a project window to connect Claude Code, Codex CLI, ChatGPT, Cursor, or API-key providers.",
                ))
                .into_any_element();
        }
        let scope = zest_core::mcp_admin::config_scope(&self.project.config_root);
        body = body.child(ui::section_heading(
            "Connected providers",
            format!("Editing {scope}. API keys stay in the system credential manager, never in zest.toml."),
        ));
        if let Some(form) = &page.form {
            body = body.child(self.key_form(form, cx));
        }
        let configured: Vec<(usize, &ProviderRow)> = page
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.state != ProviderState::NotAdded)
            .collect();
        let offers: Vec<(usize, &ProviderRow)> = page
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.state == ProviderState::NotAdded)
            .collect();
        if configured.is_empty() {
            body = body.child(ui::empty_state(
                Icon::Sparkles,
                "No providers connected yet",
                "Add a provider below to start chatting. If you already use Claude Code, Codex, or Cursor, Zest reuses that sign-in.",
            ));
        } else {
            let mut list = div().flex().flex_col().gap_2();
            for (index, row) in configured {
                list = list.child(self.provider_row(index, row, cx));
            }
            body = body.child(list);
        }
        if !offers.is_empty() {
            let mut add_list = div().flex().flex_col().gap_2();
            for (index, row) in offers {
                add_list = add_list.child(self.provider_row(index, row, cx));
            }
            body = body.child(
                div()
                    .border_t_1()
                    .border_color(theme::alpha(theme::border(), 0.6))
                    .pt_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(ui::section_heading(
                        "Add a provider",
                        "Connect another local CLI or API-key provider to switch between models in any chat.",
                    ))
                    .child(add_list),
            );
        }
        body.into_any_element()
    }

    fn provider_row(&self, index: usize, row: &ProviderRow, cx: &mut Context<Self>) -> AnyElement {
        let status = match row.state {
            ProviderState::Ready => "Ready",
            ProviderState::NeedsSignIn if row.takes_key => "Needs a key",
            ProviderState::NeedsSignIn => "Needs sign-in",
            ProviderState::Unknown => "Unverified",
            ProviderState::NotAdded => "Not added",
        };
        let mut actions = div().flex().items_center().gap_2();
        let id = row.id.clone();
        if row.state == ProviderState::NotAdded {
            let choice = row.choice.expect("offers have a kind");
            actions = actions.child(ui::button(
                SharedString::from(format!("provider-add-{index}")),
                Variant::Outline,
                Some(Icon::Plus),
                format!("Add {}", row.label),
                cx.listener(move |ws, _, w, cx| {
                    if choice.needs_key() {
                        ws.open_key_form(Some(choice), String::new(), w, cx);
                    } else {
                        ws.add_provider(choice, ProviderForm::default(), cx);
                    }
                }),
            ));
        } else {
            if signs_in(row.choice) && row.state != ProviderState::Ready {
                let login = id.clone();
                let busy = self.login.is_some();
                actions = actions.child(ui::button(
                    SharedString::from(format!("provider-sign-in-{index}")),
                    Variant::Primary,
                    busy.then_some(Icon::LoaderCircle),
                    if busy { "Signing in…" } else { "Sign in" },
                    cx.listener(move |ws, _, _, cx| ws.start_login_for(login.clone(), cx)),
                ));
            }
            if row.takes_key {
                let key_for = id.clone();
                actions = actions.child(ui::button(
                    SharedString::from(format!("provider-key-{index}")),
                    Variant::Outline,
                    None,
                    if row.state == ProviderState::Ready { "Replace key" } else { "Save key" },
                    cx.listener(move |ws, _, w, cx| ws.open_key_form(None, key_for.clone(), w, cx)),
                ));
            }
            if !row.is_default {
                let default = id.clone();
                actions = actions.child(ui::button(
                    SharedString::from(format!("provider-default-{index}")),
                    Variant::Ghost,
                    None,
                    "Use for new chats",
                    cx.listener(move |ws, _, _, cx| ws.use_for_new_chats(default.clone(), cx)),
                ));
            }
        }
        let title = if row.state == ProviderState::NotAdded { row.label.clone() } else { format!("{}  ·  {}", row.label, row.id) };
        ui::card()
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(26.))
                            .rounded(px(7.))
                            .bg(rgb(theme::secondary()))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .child(theme::provider_icon(&row.id, row.choice, 14.)),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(ui::pill(None, status))
                    .when(row.is_default, |d| d.child(ui::pill(Some(Icon::CircleCheck), "New chats")))
                    .child(div().flex_1())
                    .child(actions),
            )
            .child(div().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(format!("{}. {}", row.method, row.detail)))
            .into_any_element()
    }

    fn key_form(&self, form: &KeyForm, cx: &mut Context<Self>) -> AnyElement {
        let labeled = |label: &'static str, input: &Entity<Composer>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(label))
                .child(ui::card().px_2().py_1().child(input.clone()))
        };
        let title = match form.adding {
            Some(choice) => format!("Add {}", choice.label()),
            None => format!("API key for {}", form.id),
        };
        let mut card = ui::card().p_3().flex().flex_col().gap_3().child(ui::section_title(title));
        if form.adding == Some(ProviderChoice::OpenAiCompatible) {
            card = card.child(labeled("Endpoint", &form.base_url));
        }
        card = card.child(labeled("API key", &form.key));
        if form.adding.is_some() {
            card = card.child(labeled("Model", &form.model));
        }
        card.child(
            div()
                .flex()
                .gap_2()
                .child(ui::button("provider-form-save", Variant::Primary, None, if form.adding.is_some() { "Add provider" } else { "Save key" }, cx.listener(|ws, _, _, cx| ws.submit_key_form(cx))))
                .child(ui::button("provider-form-cancel", Variant::Ghost, None, "Cancel", cx.listener(|ws, _, _, cx| {
                    ws.providers_page.form = None;
                    cx.notify();
                }))),
        )
        .into_any_element()
    }
}
