//! Pre-GPUI 1:1 onboarding screen (`ProviderPicker`, `ApiProviderForm`, `WaitingScreen`
//! inside `AuthShell`), rendered natively in GPUI.
//!
//! Shown when Zest launches without `--project` or `--demo`. Lets the user pick
//! an existing CLI/ChatGPT sign-in or add an API-key provider (Anthropic, DeepSeek,
//! OpenAI, or Custom), optionally choose a project folder, and continue straight
//! into the workspace inside the same window.

use super::*;
use crate::composer::Composer;
use crate::theme::{self, Icon};
use crate::ui::{self, Variant};
use std::path::{Path, PathBuf};
use zest_core::auth::{self, LoginPoll, LoginProcess};
use zest_core::provider_setup::{self, ProviderChoice, ProviderForm, ProviderRow, ProviderState};
use zest_core::{Config, ProviderConfig};

const MAX_RECENT_PROJECTS: usize = 8;
const KNOWN_WORKSPACES_FILE: &str = "known-workspaces.json";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApiPreset {
    Anthropic,
    DeepSeek,
    OpenAi,
    Custom,
}

impl ApiPreset {
    const ALL: [ApiPreset; 4] = [
        ApiPreset::Anthropic,
        ApiPreset::DeepSeek,
        ApiPreset::OpenAi,
        ApiPreset::Custom,
    ];

    fn label(self) -> &'static str {
        match self {
            ApiPreset::Anthropic => "Anthropic",
            ApiPreset::DeepSeek => "DeepSeek",
            ApiPreset::OpenAi => "OpenAI",
            ApiPreset::Custom => "Custom",
        }
    }

    fn default_id(self) -> &'static str {
        match self {
            ApiPreset::Anthropic => "anthropic",
            ApiPreset::DeepSeek => "deepseek",
            ApiPreset::OpenAi => "openai",
            ApiPreset::Custom => "custom",
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            ApiPreset::Anthropic => "https://api.anthropic.com",
            ApiPreset::DeepSeek => "https://api.deepseek.com",
            ApiPreset::OpenAi => "https://api.openai.com/v1",
            ApiPreset::Custom => "",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            ApiPreset::Anthropic => "claude-opus-5",
            ApiPreset::DeepSeek => "deepseek-flash",
            ApiPreset::OpenAi => "gpt-5",
            ApiPreset::Custom => "",
        }
    }

    fn models_summary(self) -> &'static str {
        match self {
            ApiPreset::DeepSeek => "deepseek-flash, deepseek-v4-pro",
            _ => "",
        }
    }

    fn choice(self) -> ProviderChoice {
        match self {
            ApiPreset::Anthropic => ProviderChoice::Anthropic,
            ApiPreset::DeepSeek | ApiPreset::OpenAi | ApiPreset::Custom => {
                ProviderChoice::OpenAiCompatible
            }
        }
    }
}

struct ApiFormState {
    preset: ApiPreset,
    id: Entity<Composer>,
    base_url: Entity<Composer>,
    model: Entity<Composer>,
    key: Entity<Composer>,
    error: Option<String>,
}

struct WaitingState {
    title: String,
    body: String,
    error: Option<String>,
}

struct PickerError {
    message: String,
    workspace: bool,
}

pub(super) struct Launcher {
    config_root: PathBuf,
    registry_dir: Option<PathBuf>,
    config: Config,
    rows: Vec<ProviderRow>,
    selected_id: Option<String>,
    workspace_path: Option<PathBuf>,
    error: Option<PickerError>,
    api_form: Option<ApiFormState>,
    inline_key: Entity<Composer>,
    key_error: Option<String>,
    waiting: Option<WaitingState>,
    login: Option<LoginProcess>,
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(mut login) = self.login.take() {
            let _ = login.kill();
        }
    }
}

fn default_registry_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("zest"))
}

fn make_input(
    cx: &mut Context<Launcher>,
    placeholder: &'static str,
    initial: &str,
) -> Entity<Composer> {
    let initial = initial.to_string();
    cx.new(|cx| {
        let mut input = Composer::new(cx);
        input.placeholder = placeholder;
        input.text = initial;
        input
    })
}

fn signs_in(choice: Option<ProviderChoice>) -> bool {
    matches!(
        choice,
        Some(ProviderChoice::ClaudeCode | ProviderChoice::CodexCli | ProviderChoice::ChatGpt)
    )
}

fn normalize_project_key(path: &Path) -> String {
    let display = zest_core::display_path(path);
    if cfg!(windows) {
        display.to_ascii_lowercase()
    } else {
        display
    }
}

/// Load existing project directories from `<registry_dir>/known-workspaces.json`.
pub(super) fn load_recent_projects(registry_dir: &Path) -> Vec<PathBuf> {
    let raw = match std::fs::read(registry_dir.join(KNOWN_WORKSPACES_FILE)) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    let parsed: Vec<PathBuf> = serde_json::from_slice(&raw).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    parsed
        .into_iter()
        .filter_map(|p| {
            let canonical = std::fs::canonicalize(&p).ok()?;
            if !canonical.is_dir() {
                return None;
            }
            seen.insert(normalize_project_key(&canonical))
                .then_some(canonical)
        })
        .take(MAX_RECENT_PROJECTS)
        .collect()
}

/// Record a project directory at the top of `<registry_dir>/known-workspaces.json`.
pub(super) fn record_recent_project(registry_dir: &Path, path: &Path) {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = normalize_project_key(&canonical);
    let mut existing = load_recent_projects(registry_dir);
    existing.retain(|p| normalize_project_key(p) != key);
    existing.insert(0, canonical);
    existing.truncate(MAX_RECENT_PROJECTS);
    let _ = zest_core::fsutil::atomic_write_json(
        &registry_dir.join(KNOWN_WORKSPACES_FILE),
        &existing,
    );
}

/// Same short folder format as pre-GPUI `ProviderPicker.tsx`: last two path segments.
fn short_root(path: &Path) -> String {
    let cleaned = zest_core::display_path(path);
    let normalized = cleaned.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 {
        cleaned
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

/// Providers shown in the main listbox: configured providers plus unadded CLI/ChatGPT
/// providers (unadded API-key providers are added via "Add a provider with an API key",
/// matching the original `ProviderPicker.tsx`).
fn picker_rows(config: &Config) -> Vec<ProviderRow> {
    provider_setup::rows(config)
        .into_iter()
        .filter(|r| {
            r.state != ProviderState::NotAdded
                || !r.choice.is_some_and(ProviderChoice::needs_key)
        })
        .collect()
}

fn default_selection(rows: &[ProviderRow]) -> Option<String> {
    rows.iter()
        .find(|r| r.is_default && matches!(r.state, ProviderState::Ready | ProviderState::Unknown))
        .or_else(|| {
            rows.iter()
                .find(|r| matches!(r.state, ProviderState::Ready | ProviderState::Unknown))
        })
        .or_else(|| rows.first())
        .map(|r| r.id.clone())
}

fn row_is_ready(row: &ProviderRow) -> bool {
    matches!(row.state, ProviderState::Ready | ProviderState::Unknown)
}

fn cli_enable_copy(choice: ProviderChoice) -> Option<(&'static str, &'static str, &'static str)> {
    match choice {
        ProviderChoice::ClaudeCode => Some((
            "Use Claude Code subscription",
            "Zest will use your Claude Code CLI session as the parent agent, with no delegated worker involved.",
            "Enable Claude Code",
        )),
        ProviderChoice::CodexCli => Some((
            "Use Codex CLI subscription",
            "Zest will use your Codex CLI session as the parent agent, with no delegated worker involved.",
            "Enable Codex CLI",
        )),
        ProviderChoice::Cursor => Some((
            "Use Cursor subscription",
            "Zest will use your Cursor CLI session as the parent agent, with no delegated worker involved.",
            "Enable Cursor",
        )),
        _ => None,
    }
}

impl Launcher {
    fn new(cx: &mut Context<Self>) -> Self {
        let registry_dir = default_registry_dir();
        let config_root = registry_dir.clone().unwrap_or_default();
        let config = Config::find(&config_root)
            .unwrap_or_else(|_| Config::parse("").expect("empty config"));
        let rows = picker_rows(&config);
        let selected_id = default_selection(&rows);
        let workspace_path = registry_dir
            .as_deref()
            .and_then(|dir| load_recent_projects(dir).into_iter().next());
        let inline_key = make_input(cx, "Paste API key", "");
        Self {
            config_root,
            registry_dir,
            config,
            rows,
            selected_id,
            workspace_path,
            error: None,
            api_form: None,
            inline_key,
            key_error: None,
            waiting: None,
            login: None,
        }
    }

    fn selected_row(&self) -> Option<&ProviderRow> {
        let id = self.selected_id.as_deref()?;
        self.rows.iter().find(|r| r.id == id)
    }

    fn reload_providers(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        match Config::find(&self.config_root) {
            Ok(config) => {
                self.config = config;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(PickerError {
                    message: e.to_string(),
                    workspace: false,
                });
            }
        }
        self.rows = picker_rows(&self.config);
        if let Some(id) = select {
            self.selected_id = Some(id);
        } else if self
            .selected_id
            .as_deref()
            .is_none_or(|id| !self.rows.iter().any(|r| r.id == id))
        {
            self.selected_id = default_selection(&self.rows);
        }
        cx.notify();
    }

    fn open_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| {
            let selected = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(err)) => {
                    let _ = this.update(cx, |view, cx| {
                        view.error = Some(PickerError {
                            message: err.to_string(),
                            workspace: true,
                        });
                        cx.notify();
                    });
                    return;
                }
                _ => return,
            };
            let Some(path) = selected else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                if path.is_dir() {
                    view.workspace_path = Some(path);
                    if view.error.as_ref().is_some_and(|e| e.workspace) {
                        view.error = None;
                    }
                } else {
                    view.error = Some(PickerError {
                        message: "Choose a valid project directory.".into(),
                        workspace: true,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn enable_cli(&mut self, choice: ProviderChoice, cx: &mut Context<Self>) {
        match provider_setup::add(&self.config_root, choice, ProviderForm::default()) {
            Ok(id) => {
                self.error = None;
                self.reload_providers(Some(id), cx);
            }
            Err(message) => {
                self.error = Some(PickerError {
                    message,
                    workspace: false,
                });
                cx.notify();
            }
        }
    }

    fn open_api_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let preset = ApiPreset::Anthropic;
        let form = ApiFormState {
            preset,
            id: make_input(cx, "e.g. local-llm", preset.default_id()),
            base_url: make_input(cx, "https://api.example.com/v1", preset.base_url()),
            model: make_input(cx, "model-name", preset.default_model()),
            key: make_input(cx, "sk-…", ""),
            error: None,
        };
        window.focus(&form.key.read(cx).focus.clone(), cx);
        self.api_form = Some(form);
        cx.notify();
    }

    fn select_api_preset(&mut self, preset: ApiPreset, cx: &mut Context<Self>) {
        let Some(form) = self.api_form.as_mut() else {
            return;
        };
        form.preset = preset;
        form.error = None;
        form.id.update(cx, |c, cx| {
            c.text = preset.default_id().into();
            cx.notify();
        });
        form.base_url.update(cx, |c, cx| {
            c.text = preset.base_url().into();
            cx.notify();
        });
        form.model.update(cx, |c, cx| {
            c.text = preset.default_model().into();
            cx.notify();
        });
        cx.notify();
    }

    fn submit_api_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.api_form.as_ref() else {
            return;
        };
        let preset = form.preset;
        let read = |e: &Entity<Composer>| e.read(cx).text.trim().to_string();
        let (id, base_url, model, key) = if preset == ApiPreset::Custom {
            (
                read(&form.id),
                read(&form.base_url),
                read(&form.model),
                read(&form.key),
            )
        } else {
            (
                preset.default_id().to_string(),
                preset.base_url().to_string(),
                preset.default_model().to_string(),
                read(&form.key),
            )
        };
        let request = ProviderForm {
            id,
            model,
            base_url,
            key,
        };
        match provider_setup::add(&self.config_root, preset.choice(), request) {
            Ok(added_id) => {
                self.api_form = None;
                self.reload_providers(Some(added_id), cx);
            }
            Err(err) => {
                if let Some(form) = self.api_form.as_mut() {
                    form.error = Some(err);
                }
                cx.notify();
            }
        }
    }

    fn save_inline_key(&mut self, id: String, cx: &mut Context<Self>) {
        let key = self.inline_key.read(cx).text.trim().to_string();
        if key.is_empty() {
            return;
        }
        match provider_setup::save_key(&self.config, &id, &key) {
            Ok(()) => {
                self.key_error = None;
                self.inline_key.update(cx, |c, cx| {
                    c.text.clear();
                    cx.notify();
                });
                self.reload_providers(Some(id), cx);
            }
            Err(err) => {
                self.key_error = Some(err);
                cx.notify();
            }
        }
    }

    fn start_connect(&mut self, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let id = if row.state == ProviderState::NotAdded {
            let Some(choice) = row.choice else {
                return;
            };
            match provider_setup::add(&self.config_root, choice, ProviderForm::default()) {
                Ok(new_id) => {
                    self.reload_providers(Some(new_id.clone()), cx);
                    new_id
                }
                Err(message) => {
                    self.error = Some(PickerError {
                        message,
                        workspace: false,
                    });
                    cx.notify();
                    return;
                }
            }
        } else {
            row.id
        };

        let result = match self.config.providers.get(&id) {
            Some(ProviderConfig::CodexOAuth { credential, .. }) => auth::start_codex_oauth_login(
                credential
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&id),
            ),
            Some(ProviderConfig::CodexCli { .. }) => auth::start_codex_cli_login(),
            Some(ProviderConfig::ClaudeCode { .. }) => auth::start_claude_code_login(),
            _ => return,
        };

        match result {
            Ok(login) => {
                self.waiting = Some(WaitingState {
                    title: login.spawn.browser_title.to_string(),
                    body: login.spawn.browser_body.to_string(),
                    error: None,
                });
                self.login = Some(login);
                self.error = None;
                cx.notify();
            }
            Err(message) => {
                self.error = Some(PickerError {
                    message,
                    workspace: false,
                });
                cx.notify();
                return;
            }
        }

        let target_id = id.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let keep_waiting = this
                    .update(cx, |launcher, cx| {
                        let Some(login) = launcher.login.as_mut() else {
                            return false;
                        };
                        match login.poll_status() {
                            Ok(LoginPoll::Running) => return true,
                            Ok(LoginPoll::Succeeded) => {
                                launcher.login = None;
                                launcher.waiting = None;
                                launcher.reload_providers(Some(target_id.clone()), cx);
                            }
                            Ok(LoginPoll::Failed(detail)) => {
                                launcher.login = None;
                                if let Some(w) = launcher.waiting.as_mut() {
                                    w.error = Some(detail);
                                }
                                cx.notify();
                            }
                            Err(e) => {
                                launcher.login = None;
                                if let Some(w) = launcher.waiting.as_mut() {
                                    w.error = Some(e.to_string());
                                }
                                cx.notify();
                            }
                        }
                        false
                    })
                    .unwrap_or(false);
                if !keep_waiting {
                    break;
                }
            }
        })
        .detach();
    }

    fn cancel_waiting(&mut self, cx: &mut Context<Self>) {
        if let Some(mut login) = self.login.take() {
            let _ = login.kill();
        }
        self.waiting = None;
        self.reload_providers(self.selected_id.clone(), cx);
    }

    fn continue_to_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.selected_row().cloned() else {
            return;
        };
        if !row_is_ready(&selected) {
            return;
        }
        if !selected.is_default {
            if let Err(message) = provider_setup::make_default(&self.config_root, &selected.id) {
                self.error = Some(PickerError {
                    message,
                    workspace: false,
                });
                cx.notify();
                return;
            }
        }

        let (target_path, record_recent) = match self.workspace_path.clone() {
            Some(path) => (path, true),
            None => match recent::free_chats_root() {
                Some(root) => {
                    if let Err(err) = std::fs::create_dir_all(&root) {
                        self.error = Some(PickerError {
                            message: format!("Could not initialize workspace: {err}"),
                            workspace: true,
                        });
                        cx.notify();
                        return;
                    }
                    (root, false)
                }
                None => {
                    self.error = Some(PickerError {
                        message: "Choose a project folder to continue.".into(),
                        workspace: true,
                    });
                    cx.notify();
                    return;
                }
            },
        };

        let current_window = window.window_handle();
        let entity = cx.entity().downgrade();
        let registry = self.registry_dir.clone();
        cx.defer(move |cx| match Project::open(target_path.clone(), false) {
            Ok(project) => {
                if record_recent && let Some(dir) = registry.as_deref() {
                    record_recent_project(dir, &project.path);
                }
                let _ = current_window.update(cx, |_, w, cx| {
                    w.replace_root(cx, |w, cx| Workspace::new(project, w, cx));
                });
            }
            Err(err) => {
                let _ = entity.update(cx, |launcher, cx| {
                    launcher.error = Some(PickerError {
                        message: err.to_string(),
                        workspace: true,
                    });
                    cx.notify();
                });
            }
        });
    }

    fn render_waiting(&self, state: &WaitingState, cx: &mut Context<Self>) -> Div {
        let has_error = state.error.is_some();
        div()
            .w_full()
            .max_w(px(400.))
            .flex()
            .flex_col()
            .child(
                div()
                    .mb_6()
                    .flex()
                    .flex_col()
                    .child(div().mb_4().child(theme::zest_mark(28.)))
                    .child(
                        div()
                            .mb(px(6.))
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(state.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(theme::muted_foreground()))
                            .child(state.body.clone()),
                    ),
            )
            .child(
                div()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::alpha(theme::border(), 0.7))
                    .bg(theme::alpha(theme::card(), 0.4))
                    .px(px(14.))
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_size(px(13.))
                    .text_color(rgb(theme::muted_foreground()))
                    .child(if has_error {
                        theme::icon(Icon::CircleAlert, 15.)
                            .text_color(rgb(theme::destructive()))
                            .into_any_element()
                    } else {
                        theme::spinner("waiting-signin-spinner", 15.).into_any_element()
                    })
                    .child(if has_error {
                        "Sign-in did not complete."
                    } else {
                        "Waiting for sign-in to complete…"
                    }),
            )
            .when_some(state.error.clone(), |d, err| {
                d.child(
                    div()
                        .mt_3()
                        .text_size(px(12.))
                        .text_color(rgb(theme::destructive()))
                        .child(err),
                )
            })
            .child(
                div()
                    .mt_6()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(ui::button(
                        "waiting-cancel",
                        Variant::Outline,
                        None,
                        if has_error {
                            "Back to providers"
                        } else {
                            "Cancel"
                        },
                        cx.listener(|l, _, _, cx| l.cancel_waiting(cx)),
                    )),
            )
    }

    fn render_api_form(&self, form: &ApiFormState, cx: &mut Context<Self>) -> Div {
        let preset = form.preset;
        let field_box = |label: String, hint: Option<&'static str>, input: &Entity<Composer>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme::muted_foreground()))
                        .child(label),
                )
                .child(
                    div()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::alpha(theme::border(), 0.8))
                        .bg(rgb(theme::canvas()))
                        .px(px(10.))
                        .py(px(6.))
                        .child(input.clone()),
                )
                .when_some(hint, |d, text| {
                    d.child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::alpha(theme::muted_foreground(), 0.85))
                            .child(text),
                    )
                })
        };

        let mut tabs = div()
            .mb_3()
            .flex()
            .gap_1()
            .rounded(px(8.))
            .border_1()
            .border_color(theme::alpha(theme::border(), 0.6))
            .bg(theme::alpha(theme::canvas(), 0.5))
            .p_1();
        for item in ApiPreset::ALL {
            let active = item == preset;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("api-preset-{}", item.default_id())))
                    .role(Role::Tab)
                    .tab_index(0)
                    .cursor_pointer()
                    .flex_1()
                    .px_2()
                    .py(px(6.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .when(active, |d| {
                        d.bg(rgb(theme::secondary()))
                            .text_color(rgb(theme::foreground()))
                    })
                    .when(!active, |d| {
                        d.text_color(rgb(theme::muted_foreground()))
                            .hover(|s| s.text_color(rgb(theme::foreground())))
                    })
                    .on_click(cx.listener(move |l, _, _, cx| l.select_api_preset(item, cx)))
                    .child(item.label()),
            );
        }

        let heading = if preset == ApiPreset::Custom {
            "Add API provider".to_string()
        } else {
            format!("Connect {}", preset.label())
        };

        let mut body = div()
            .mt_4()
            .rounded(px(8.))
            .border_1()
            .border_color(theme::alpha(theme::border(), 0.7))
            .bg(theme::alpha(theme::secondary(), 0.25))
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(6.))
                            .bg(theme::alpha(theme::primary(), 0.15))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .child(theme::icon(Icon::Lock, 14.).text_color(rgb(theme::primary()))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(heading),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme::muted_foreground()))
                                    .child("Use your own API key. It stays in your operating system's secure storage."),
                            ),
                    ),
            )
            .child(tabs);

        if preset == ApiPreset::Custom {
            body = body.child(
                div()
                    .mb_3()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(field_box(
                        "Provider id".into(),
                        Some("A short name used to identify this provider."),
                        &form.id,
                    ))
                    .child(field_box("Base URL".into(), None, &form.base_url))
                    .child(field_box("Default model".into(), None, &form.model)),
            );
        } else {
            let info_row = |label: &'static str, value: &'static str| {
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .text_size(px(11.))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_color(rgb(theme::muted_foreground()))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_color(rgb(theme::foreground()))
                            .child(value),
                    )
            };
            let mut summary = div()
                .mb_3()
                .rounded(px(8.))
                .border_1()
                .border_color(theme::alpha(theme::border(), 0.6))
                .bg(theme::alpha(theme::card(), 0.5))
                .px_3()
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(info_row("Endpoint", preset.base_url()))
                .child(info_row("Default", preset.default_model()));
            if !preset.models_summary().is_empty() {
                summary = summary.child(info_row("Models", preset.models_summary()));
            }
            summary = summary.child(
                div()
                    .pt_1()
                    .text_size(px(10.))
                    .text_color(theme::alpha(theme::muted_foreground(), 0.85))
                    .child("Switch to Custom to change the endpoint or model names."),
            );
            body = body.child(summary);
        }

        let key_label = if preset == ApiPreset::Custom {
            "API key".to_string()
        } else {
            format!("{} API key", preset.label())
        };
        body = body.child(field_box(
            key_label,
            Some("Your key is never saved in zest.toml."),
            &form.key,
        ));

        if let Some(err) = &form.error {
            body = body.child(
                div()
                    .mt_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme::alpha(theme::destructive(), 0.4))
                    .bg(theme::alpha(theme::destructive(), 0.08))
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(11.))
                    .text_color(rgb(theme::destructive()))
                    .child(err.clone()),
            );
        }

        body.child(
            div()
                .mt_3()
                .flex()
                .justify_end()
                .gap_2()
                .child(ui::button(
                    "api-form-cancel",
                    Variant::Ghost,
                    None,
                    "Cancel",
                    cx.listener(|l, _, _, cx| {
                        l.api_form = None;
                        cx.notify();
                    }),
                ))
                .child(ui::button(
                    "api-form-submit",
                    Variant::Primary,
                    None,
                    if preset == ApiPreset::Custom {
                        "Save provider"
                    } else {
                        "Save & connect"
                    },
                    cx.listener(|l, _, _, cx| l.submit_api_form(cx)),
                )),
        )
    }

    fn render_picker(&self, cx: &mut Context<Self>) -> Div {
        let selected = self.selected_row();
        let ready = selected.is_some_and(row_is_ready);

        // Header
        let header = div()
            .mb_6()
            .flex()
            .flex_col()
            .child(div().mb_4().child(theme::zest_mark(28.)))
            .child(
                div()
                    .mb(px(6.))
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Choose a provider"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(theme::muted_foreground()))
                    .child("Use an existing sign-in, or connect a provider with your own API key. Zest never asks for your password."),
            );

        // Project folder (optional)
        let folder_label = self
            .workspace_path
            .as_deref()
            .map(short_root)
            .unwrap_or_else(|| "No project selected".into());
        let folder_row = div()
            .mb_5()
            .pb_4()
            .border_b_1()
            .border_color(theme::alpha(theme::border(), 0.6))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme::muted_foreground()))
                            .child("PROJECT FOLDER (OPTIONAL)"),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(12.))
                            .text_color(theme::alpha(theme::foreground(), 0.85))
                            .child(folder_label),
                    ),
            )
            .child(ui::button(
                "pick-project",
                Variant::Outline,
                Some(Icon::FolderOpen),
                "Open",
                cx.listener(|l, _, _, cx| l.open_folder(cx)),
            ));

        // Providers listbox
        let mut list = div()
            .rounded(px(8.))
            .border_1()
            .border_color(theme::alpha(theme::border(), 0.7))
            .bg(theme::alpha(theme::card(), 0.4))
            .overflow_hidden()
            .flex()
            .flex_col();

        let total = self.rows.len();
        for (index, row) in self.rows.iter().enumerate() {
            let is_selected = self.selected_id.as_deref() == Some(row.id.as_str());
            let row_id = row.id.clone();
            let usable_ready = row.state == ProviderState::Ready;
            let detail = match row.state {
                ProviderState::Ready => row.method.clone(),
                ProviderState::Unknown => "Connection status unavailable".into(),
                _ => row.detail.clone(),
            };
            let status_label = match row.state {
                ProviderState::Ready => "Ready",
                ProviderState::NeedsSignIn if row.takes_key => "Needs key",
                ProviderState::NeedsSignIn => "Sign in",
                ProviderState::Unknown => "Unverified",
                ProviderState::NotAdded => "Configure",
            };
            let last = index + 1 == total;
            list = list.child(
                div()
                    .id(SharedString::from(format!("provider-option-{index}")))
                    .role(Role::Button)
                    .aria_label(SharedString::from(row.label.clone()))
                    .tab_index(0)
                    .cursor_pointer()
                    .px(px(14.))
                    .py_3()
                    .when(!last, |d| {
                        d.border_b_1()
                            .border_color(theme::alpha(theme::border(), 0.5))
                    })
                    .when(is_selected, |d| d.bg(theme::alpha(theme::accent(), 0.75)))
                    .hover(|s| s.bg(theme::alpha(theme::accent(), 0.55)))
                    .on_click(cx.listener(move |l, _, _, cx| {
                        l.selected_id = Some(row_id.clone());
                        l.key_error = None;
                        cx.notify();
                    }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(theme::provider_icon(&row.id, row.choice, 16.))
                                    .child(row.label.clone())
                                    .when(is_selected, |d| {
                                        d.child(
                                            theme::icon(Icon::Check, 12.)
                                                .text_color(rgb(theme::primary())),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme::muted_foreground()))
                                    .child(detail),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(if usable_ready {
                                theme::primary()
                            } else {
                                theme::muted_foreground()
                            }))
                            .child(status_label),
                    ),
            );
        }

        let mut shell = div()
            .w_full()
            .max_w(px(400.))
            .flex()
            .flex_col()
            .child(header)
            .child(folder_row);

        // Workspace error box
        if let Some(err) = self.error.as_ref().filter(|e| e.workspace) {
            shell = shell.child(
                div()
                    .mb_5()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::alpha(theme::destructive(), 0.4))
                    .bg(theme::alpha(theme::destructive(), 0.1))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme::destructive()))
                            .child("This folder cannot be used as a project"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::alpha(theme::foreground(), 0.85))
                            .child(err.message.clone()),
                    )
                    .child(ui::button(
                        "choose-different-folder",
                        Variant::Outline,
                        Some(Icon::FolderOpen),
                        "Choose a different folder",
                        cx.listener(|l, _, _, cx| l.open_folder(cx)),
                    )),
            );
        }

        shell = shell
            .child(
                div()
                    .mb_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::muted_foreground()))
                    .child("AVAILABLE ON THIS MACHINE"),
            )
            .child(list);

        // Non-workspace error box
        if let Some(err) = self.error.as_ref().filter(|e| !e.workspace) {
            shell = shell.child(
                div()
                    .mt_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::alpha(theme::destructive(), 0.4))
                    .bg(theme::alpha(theme::destructive(), 0.1))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(theme::destructive()))
                            .child(err.message.clone()),
                    )
                    .child(ui::button(
                        "retry-providers",
                        Variant::Outline,
                        None,
                        "Try again",
                        cx.listener(|l, _, _, cx| l.reload_providers(l.selected_id.clone(), cx)),
                    )),
            );
        }

        // Unconfigured CLI subscription enable card
        if let Some(sel) = selected
            && sel.state == ProviderState::NotAdded
            && let Some(choice) = sel.choice
            && let Some((title, body, cta)) = cli_enable_copy(choice)
        {
            shell = shell.child(
                div()
                    .mt_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::alpha(theme::border(), 0.7))
                    .bg(theme::alpha(theme::card(), 0.4))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(theme::muted_foreground()))
                            .child(body),
                    )
                    .child(ui::button(
                        "enable-cli-provider",
                        Variant::Primary,
                        None,
                        cta,
                        cx.listener(move |l, _, _, cx| l.enable_cli(choice, cx)),
                    )),
            );
        }

        // Add API key provider button / inline ApiProviderForm
        if let Some(form) = &self.api_form {
            shell = shell.child(self.render_api_form(form, cx));
        } else {
            shell = shell.child(
                div().mt_3().child(
                    ui::button(
                        "add-api-provider",
                        Variant::Outline,
                        Some(Icon::Plus),
                        "Add a provider with an API key",
                        cx.listener(|l, _, w, cx| l.open_api_form(w, cx)),
                    )
                    .w_full()
                    .justify_center(),
                ),
            );
        }

        // Selected configured API-key provider inline key updater
        if let Some(sel) = selected
            && sel.takes_key
            && sel.state != ProviderState::NotAdded
        {
            let save_id = sel.id.clone();
            shell = shell.child(
                div()
                    .mt_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::alpha(theme::border(), 0.7))
                    .bg(theme::alpha(theme::card(), 0.4))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("{} API key", sel.label)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(theme::muted_foreground()))
                            .child("Your key is stored securely by your operating system."),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme::alpha(theme::border(), 0.8))
                                    .bg(rgb(theme::canvas()))
                                    .px(px(10.))
                                    .py(px(6.))
                                    .child(self.inline_key.clone()),
                            )
                            .child(ui::button(
                                "save-inline-key",
                                Variant::Primary,
                                None,
                                "Save",
                                cx.listener(move |l, _, _, cx| {
                                    l.save_inline_key(save_id.clone(), cx);
                                }),
                            )),
                    )
                    .when_some(self.key_error.clone(), |d, err| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(theme::destructive()))
                                .child(err),
                        )
                    }),
            );
        }

        // Footer: Connect/Reconnect + Continue
        let can_connect = selected.is_some_and(|s| signs_in(s.choice));
        let connect_label = if selected.is_some_and(|s| s.state == ProviderState::Ready) {
            "Reconnect"
        } else {
            "Connect"
        };
        let mut footer = div().mt_6().flex().justify_end().gap_2();
        if can_connect {
            footer = footer.child(ui::button(
                "picker-connect",
                Variant::Outline,
                None,
                connect_label,
                cx.listener(|l, _, _, cx| l.start_connect(cx)),
            ));
        }
        footer = footer.child(
            ui::button(
                "picker-continue",
                Variant::Primary,
                None,
                "Continue",
                cx.listener(|l, _, w, cx| l.continue_to_workspace(w, cx)),
            )
            .when(!ready, |d| d.opacity(0.45)),
        );

        shell.child(footer)
    }
}

impl Render for Launcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.waiting {
            Some(waiting) => self.render_waiting(waiting, cx),
            None => self.render_picker(cx),
        };
        div()
            .id("auth-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(theme::canvas()))
            .font_family(sans())
            .text_color(rgb(fg()))
            .child(
                div()
                    .w_full()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px_6()
                    .py_8()
                    .child(content),
            )
    }
}

pub(super) fn launch_picker(cx: &mut App) -> Result<(), String> {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: !NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed),
            titlebar: Some(TitlebarOptions {
                title: Some("Zest".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        |_, cx| cx.new(Launcher::new),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_row(id: &str, state: ProviderState) -> ProviderRow {
        ProviderRow {
            id: id.into(),
            label: id.into(),
            method: "API key".into(),
            choice: Some(ProviderChoice::Anthropic),
            state,
            detail: String::new(),
            is_default: false,
            takes_key: true,
        }
    }

    #[::core::prelude::v1::test]
    fn default_selection_prefers_ready_provider() {
        let rows = vec![
            fake_row("claude", ProviderState::NotAdded),
            fake_row("codex", ProviderState::Ready),
        ];
        assert_eq!(default_selection(&rows).as_deref(), Some("codex"));
        assert!(!row_is_ready(&rows[0]));
        assert!(row_is_ready(&rows[1]));
    }

    #[::core::prelude::v1::test]
    fn recent_projects_records_deduplicates_and_filters_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("alpha");
        let p2 = dir.path().join("beta");
        let missing = dir.path().join("gone");
        std::fs::create_dir_all(&p1).unwrap();
        std::fs::create_dir_all(&p2).unwrap();

        record_recent_project(dir.path(), &p1);
        record_recent_project(dir.path(), &p2);
        record_recent_project(dir.path(), &missing);
        record_recent_project(dir.path(), &p1);

        let loaded = load_recent_projects(dir.path());
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0], std::fs::canonicalize(&p1).unwrap());
        assert_eq!(loaded[1], std::fs::canonicalize(&p2).unwrap());
    }
}
