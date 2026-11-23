//! Customize > MCP servers: the `[mcp.<id>]` entries of the project's (or the user's)
//! `zest.toml`. Add, edit, check, turn on or off, and remove servers. The work happens in
//! `zest_core::mcp_admin`; this page only collects the form and shows the result. Changes
//! reach new chats; open chats keep the servers they started with.

use super::*;
use crate::composer::Composer;
use crate::theme::{self, Icon};
use crate::ui::{self, Variant};
use zest_core::mcp_admin::{self, McpServerForm, McpServerRow};

/// GitHub's hosted MCP server. A personal access token goes in Access token.
const GITHUB_URL: &str = "https://api.githubcopilot.com/mcp/";

pub(super) struct McpForm {
    /// The id being edited; `None` for a new server.
    editing: Option<String>,
    http: bool,
    enabled: bool,
    /// Header-to-variable mappings of an existing entry, kept because the form does not edit them.
    headers: std::collections::BTreeMap<String, String>,
    has_saved_token: bool,
    id: Entity<Composer>,
    command: Entity<Composer>,
    args: Entity<Composer>,
    env: Entity<Composer>,
    url: Entity<Composer>,
    token: Entity<Composer>,
    timeout: Entity<Composer>,
}

#[derive(Default)]
pub(super) struct McpPage {
    rows: Vec<McpServerRow>,
    scope: &'static str,
    form: Option<McpForm>,
    /// What is running: "save", or the id being checked or switched.
    busy: Option<String>,
    remove_confirm: Option<String>,
    /// The last check's message per server.
    checks: std::collections::HashMap<String, (bool, String)>,
}

fn field(cx: &mut Context<Workspace>, placeholder: &'static str, text: &str) -> Entity<Composer> {
    cx.new(|cx| {
        let mut field = Composer::new(cx);
        field.placeholder = placeholder;
        field.text = text.to_string();
        field.sel = field.text.len()..field.text.len();
        field
    })
}

fn text(field: &Entity<Composer>, cx: &App) -> String {
    field.read(cx).text.trim().to_string()
}

impl Workspace {
    /// Reads the servers again, and refreshes the project config so new chats see the change.
    pub(super) fn load_mcp(&mut self, cx: &mut Context<Self>) {
        let root = self.project.config_root.clone();
        self.mcp_page.scope = mcp_admin::config_scope(&root);
        match zest_core::Config::find(&root) {
            Ok(config) => {
                self.mcp_page.rows = mcp_admin::list(&config, &zest_core::mcp::McpCatalog::load());
            }
            Err(e) => self.toast(ToastKind::Error, e.to_string(), cx),
        }
        if let Err(e) = self.project.reload_config() {
            self.toast(ToastKind::Error, e.to_string(), cx);
        }
        cx.notify();
    }

    fn open_mcp_form(&mut self, row: Option<&McpServerRow>, github: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (id, url) = match (row, github) {
            (Some(row), _) => (row.id.clone(), row.url.clone()),
            (None, true) => ("github".to_string(), GITHUB_URL.to_string()),
            (None, false) => (String::new(), String::new()),
        };
        let form = McpForm {
            editing: row.map(|r| r.id.clone()),
            http: github || row.is_some_and(|r| !r.url.is_empty()),
            enabled: row.is_none_or(|r| r.enabled),
            headers: row.map(|r| r.headers.clone()).unwrap_or_default(),
            has_saved_token: row.is_some_and(|r| r.saved_headers.iter().any(|h| h.eq_ignore_ascii_case("authorization"))),
            id: field(cx, "Name, for example github or docs", &id),
            command: field(cx, "Command, for example npx", row.map(|r| r.command.as_str()).unwrap_or("")),
            args: field(cx, "Arguments, for example -y @modelcontextprotocol/server-filesystem .", &row.map(|r| quote_args(&r.args)).unwrap_or_default()),
            env: field(cx, "Environment variables to pass, for example GITHUB_TOKEN", &row.map(|r| r.env_vars.join(", ")).unwrap_or_default()),
            url: field(cx, "https://example.com/mcp", &url),
            token: field(cx, "Access token (sent as the Authorization header)", ""),
            timeout: field(cx, "Timeout in seconds (default 120)", &row.map(|r| r.timeout_secs.to_string()).unwrap_or_default()),
        };
        let focus = if github { form.token.clone() } else if row.is_some() { form.command.clone() } else { form.id.clone() };
        self.mcp_page.form = Some(form);
        window.focus(&focus.read(cx).focus.clone(), cx);
        cx.notify();
    }

    fn save_mcp_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.mcp_page.form.as_ref() else { return };
        let id = form.editing.clone().unwrap_or_else(|| text(&form.id, cx));
        if id.is_empty() {
            self.toast(ToastKind::Error, "Give the server a name.", cx);
            return;
        }
        let token = text(&form.token, cx);
        let timeout = text(&form.timeout, cx);
        let timeout_secs = if timeout.is_empty() {
            None
        } else {
            match timeout.parse::<u64>() {
                Ok(secs) => Some(secs),
                Err(_) => {
                    self.toast(ToastKind::Error, "The timeout must be a whole number of seconds.", cx);
                    return;
                }
            }
        };
        let submission = McpServerForm {
            id: id.clone(),
            command: if form.http { String::new() } else { text(&form.command, cx) },
            args: if form.http { Vec::new() } else { mcp_admin::parse_args(&text(&form.args, cx)) },
            url: if form.http { text(&form.url, cx) } else { String::new() },
            headers: if form.http { form.headers.clone() } else { Default::default() },
            header_secrets: if form.http && !token.is_empty() {
                std::collections::BTreeMap::from([("Authorization".to_string(), token)])
            } else {
                Default::default()
            },
            env_vars: text(&form.env, cx)
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect(),
            enabled: form.enabled,
            timeout_secs,
        };
        let (config_root, project_root) = (self.project.config_root.clone(), self.project.path.clone());
        self.mcp_page.busy = Some("save".into());
        let task = native::executor().spawn(async move { mcp_admin::save(&config_root, &project_root, submission).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |ws, cx| {
                ws.mcp_page.busy = None;
                match result {
                    Ok(()) => {
                        ws.mcp_page.form = None;
                        ws.load_mcp(cx);
                        let ready = ws.mcp_page.rows.iter().any(|r| r.id == id && !r.tools.is_empty());
                        let message = if ready { format!("Saved {id}.") } else { format!("Saved {id}. Check it to load its tools.") };
                        ws.toast(ToastKind::Success, message, cx);
                    }
                    Err(e) => ws.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn check_mcp(&mut self, id: String, cx: &mut Context<Self>) {
        let (config_root, project_root) = (self.project.config_root.clone(), self.project.path.clone());
        self.mcp_page.busy = Some(id.clone());
        let probe = id.clone();
        let task = native::executor().spawn(async move { mcp_admin::check(&config_root, &project_root, &probe).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |ws, cx| {
                ws.mcp_page.busy = None;
                match result {
                    Ok(check) => {
                        ws.mcp_page.checks.insert(id.clone(), (check.ok, check.detail.clone()));
                        ws.load_mcp(cx);
                        let kind = if check.ok { ToastKind::Success } else { ToastKind::Error };
                        ws.toast(kind, format!("{id}: {}", check.detail), cx);
                    }
                    Err(e) => ws.toast(ToastKind::Error, e, cx),
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn switch_mcp(&mut self, id: String, enabled: bool, cx: &mut Context<Self>) {
        let (config_root, project_root) = (self.project.config_root.clone(), self.project.path.clone());
        self.mcp_page.busy = Some(id.clone());
        let target = id.clone();
        let task = native::executor().spawn(async move { mcp_admin::set_enabled(&config_root, &project_root, &target, enabled).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |ws, cx| {
                ws.mcp_page.busy = None;
                if let Err(e) = result {
                    ws.toast(ToastKind::Error, e, cx);
                }
                ws.load_mcp(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_mcp(&mut self, id: String, cx: &mut Context<Self>) {
        self.mcp_page.remove_confirm = None;
        match mcp_admin::remove(&self.project.config_root, &id) {
            Ok(()) => {
                self.mcp_page.checks.remove(&id);
                self.load_mcp(cx);
                self.toast(ToastKind::Success, format!("Removed {id}."), cx);
            }
            Err(e) => self.toast(ToastKind::Error, e, cx),
        }
    }

    pub(super) fn mcp_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.mcp_page;
        let mut body = div().flex().flex_col().gap_3();
        if self.project.demo {
            return body.child(ui::hint("MCP servers are configured in project windows.")).into_any_element();
        }
        body = body.child(ui::hint(format!(
            "Editing {}. Saving or turning on a server checks it, which is what loads its tools; new chats get them, open chats keep the servers they started with.",
            page.scope
        )));
        if let Some(form) = &page.form {
            return body.child(self.mcp_form(form, cx)).into_any_element();
        }
        body = body.child(
            div()
                .flex()
                .gap_2()
                .child(ui::button("mcp-add", Variant::Primary, Some(Icon::Plus), "Add server", cx.listener(|ws, _, w, cx| ws.open_mcp_form(None, false, w, cx))))
                .when(!page.rows.iter().any(|r| r.id.eq_ignore_ascii_case("github")), |d| {
                    d.child(ui::button("mcp-github", Variant::Outline, None, "Add GitHub", cx.listener(|ws, _, w, cx| ws.open_mcp_form(None, true, w, cx))))
                }),
        );
        if page.rows.is_empty() {
            body = body.child(ui::hint("No MCP servers yet."));
        }
        for row in &page.rows {
            body = body.child(self.mcp_row(row, cx));
        }
        body.into_any_element()
    }

    fn mcp_row(&self, row: &McpServerRow, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.mcp_page;
        let busy = page.busy.as_deref() == Some(row.id.as_str());
        let target = if row.url.is_empty() {
            std::iter::once(row.command.clone()).chain(row.args.iter().cloned()).collect::<Vec<_>>().join(" ")
        } else {
            row.url.clone()
        };
        let detail = match page.checks.get(&row.id) {
            Some((false, message)) => message.clone(),
            _ => row.detail.clone(),
        };
        let (id, check_id, switch_id, edit_row, remove_id) = (row.id.clone(), row.id.clone(), row.id.clone(), row.clone(), row.id.clone());
        let mut actions = div().flex().items_center().gap_2();
        if page.remove_confirm.as_deref() == Some(row.id.as_str()) {
            actions = actions
                .child(ui::button(SharedString::from(format!("mcp-remove-yes-{id}")), Variant::Destructive, None, "Remove server", cx.listener(move |ws, _, _, cx| ws.remove_mcp(remove_id.clone(), cx))))
                .child(ui::button(SharedString::from(format!("mcp-remove-no-{id}")), Variant::Ghost, None, "Cancel", cx.listener(|ws, _, _, cx| {
                    ws.mcp_page.remove_confirm = None;
                    cx.notify();
                })));
        } else {
            let enabled = row.enabled;
            actions = actions
                .child(ui::switch(SharedString::from(format!("mcp-on-{id}")), format!("Use {id}"), enabled, cx.listener(move |ws, on: &bool, _, cx| ws.switch_mcp(switch_id.clone(), *on, cx))))
                .child(ui::button(SharedString::from(format!("mcp-check-{id}")), Variant::Outline, Some(Icon::RefreshCw), if busy { "Checking…" } else { "Check" }, cx.listener(move |ws, _, _, cx| {
                    if ws.mcp_page.busy.is_none() {
                        ws.check_mcp(check_id.clone(), cx);
                    }
                })))
                .child(ui::icon_button(SharedString::from(format!("mcp-edit-{id}")), Icon::Pencil, format!("Edit {id}"), cx.listener(move |ws, _, w, cx| ws.open_mcp_form(Some(&edit_row), false, w, cx))))
                .child(ui::icon_button(SharedString::from(format!("mcp-remove-{id}")), Icon::Trash2, format!("Remove {id}"), cx.listener(move |ws, _, _, cx| {
                    ws.mcp_page.remove_confirm = Some(id.clone());
                    cx.notify();
                })));
        }
        ui::card()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(theme::icon(Icon::Server, 14.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(row.id.clone()))
                    .child(ui::pill(None, row.status.clone()))
                    .child(div().flex_1())
                    .child(actions),
            )
            .child(div().text_size(px(12.)).font_family(theme::MONO).text_color(rgb(theme::muted_foreground())).truncate().child(target))
            .child(div().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(detail))
            .into_any_element()
    }

    fn mcp_form(&self, form: &McpForm, cx: &mut Context<Self>) -> AnyElement {
        let saving = self.mcp_page.busy.as_deref() == Some("save");
        let labeled = |label: &'static str, input: &Entity<Composer>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(12.)).text_color(rgb(theme::muted_foreground())).child(label))
                .child(ui::card().px_2().py_1().child(input.clone()))
        };
        let http = form.http;
        let mut card = ui::card().p_3().flex().flex_col().gap_3().child(ui::section_title(match &form.editing {
            Some(id) => format!("Edit {id}"),
            None => "Add an MCP server".to_string(),
        }));
        if form.editing.is_none() {
            card = card.child(labeled("Name", &form.id));
        }
        card = card.child(
            div()
                .flex()
                .gap_2()
                .child(ui::button("mcp-kind-command", if http { Variant::Ghost } else { Variant::Secondary }, None, "Command", cx.listener(|ws, _, _, cx| {
                    if let Some(form) = ws.mcp_page.form.as_mut() {
                        form.http = false;
                    }
                    cx.notify();
                })))
                .child(ui::button("mcp-kind-http", if http { Variant::Secondary } else { Variant::Ghost }, None, "HTTP URL", cx.listener(|ws, _, _, cx| {
                    if let Some(form) = ws.mcp_page.form.as_mut() {
                        form.http = true;
                    }
                    cx.notify();
                }))),
        );
        if http {
            card = card.child(labeled("URL", &form.url)).child(labeled(
                if form.has_saved_token { "Access token (saved; leave blank to keep it)" } else { "Access token" },
                &form.token,
            ));
            if !form.headers.is_empty() {
                let names = form.headers.keys().cloned().collect::<Vec<_>>().join(", ");
                card = card.child(ui::hint(format!("Also sends {names} from environment variables, as set in zest.toml.")));
            }
        } else {
            card = card.child(labeled("Command", &form.command)).child(labeled("Arguments", &form.args));
        }
        card = card
            .child(labeled("Environment variables", &form.env))
            .child(labeled("Timeout", &form.timeout))
            .child(ui::hint("Tokens are kept in the system credential manager, never in zest.toml. Environment variables are passed by name only."))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(ui::button("mcp-save", Variant::Primary, None, if saving { "Saving…" } else { "Save and check" }, cx.listener(|ws, _, _, cx| {
                        if ws.mcp_page.busy.is_none() {
                            ws.save_mcp_form(cx);
                        }
                    })))
                    .child(ui::button("mcp-cancel", Variant::Ghost, None, "Cancel", cx.listener(|ws, _, _, cx| {
                        ws.mcp_page.form = None;
                        cx.notify();
                    }))),
            );
        card.into_any_element()
    }
}

/// Joins arguments back into one line, quoting those with spaces so `parse_args` reads them
/// back the same.
fn quote_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                if arg.contains('"') { format!("'{arg}'") } else { format!("\"{arg}\"") }
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::quote_args;
    use zest_core::mcp_admin::parse_args;

    #[::core::prelude::v1::test]
    fn arguments_round_trip_through_the_form() {
        for args in [
            vec!["-y".to_string(), "@scope/server".into()],
            vec!["--root".into(), r"C:\Program Files\x".into(), "say \"hi\" now".into()],
            vec![String::new(), "plain".into()],
        ] {
            assert_eq!(parse_args(&quote_args(&args)), args);
        }
    }
}
