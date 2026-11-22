use super::*;
use crate::interaction::{Request, Response};
use std::{collections::HashMap, sync::Arc};
use zest_core::{StoredMessage, ThreadInputTarget, thread::AssistantStep};

pub struct ChatChanged;

#[cfg(test)]
mod tests {
    use super::{HashMap, StoredMessage, append_saved_rows, session_parts};

    #[test]
    fn session_line_splits_into_model_and_context() {
        assert_eq!(session_parts("demo  ·  offline  ·  high"), ("offline".into(), None));
        assert_eq!(
            session_parts("demo  ·  offline  ·  1% context  ·  127330 tokens left (estimated)"),
            ("offline".into(), Some(("1%".into(), "127K".into())))
        );
        assert_eq!(session_parts("openai"), ("openai".into(), None));
        assert_eq!(session_parts(""), ("Model".into(), None));
    }
    use serde_json::json;

    #[test]
    fn search_indexes_the_visible_answer_after_traces_across_appends() {
        let prefix = serde_json::from_value(json!([
            {"role":"user","id":"q","text":"question"},
            {"role":"assistant","id":"legacy","thinking":"reasoning","text":"legacy answer"}
        ]))
        .unwrap();
        let suffix = serde_json::from_value(json!([
            {"role":"assistant","id":"new","text":"new answer","error":"later error",
             "steps":[{"kind":"activity","id":"b"},{"kind":"tool","id":"missing"},
                      {"kind":"thinking","text":"more reasoning"},{"kind":"activity","id":"a"}],
             "providerActivity":[{"id":"a","title":"first","status":"done"},
                                 {"id":"b","title":"second","status":"done"}]},
            {"role":"assistant","id":"empty"}
        ]))
        .unwrap();
        let mut rows = Vec::new();
        let mut index = HashMap::new();
        for batch in [prefix, suffix] {
            let messages: Vec<StoredMessage> = batch;
            append_saved_rows(&messages, &mut rows, &mut index);
        }
        assert_eq!((index["q"], index["legacy"], index["new"]), (0, 2, 6));
        assert!(rows[index["new"]].trace.is_none());
        assert_eq!(rows[index["new"]].doc.units[0].text.as_ref(), "new answer");
        assert!(!index.contains_key("empty"));
    }
}
impl EventEmitter<ChatChanged> for Root {}
pub struct ChooseModel;
impl EventEmitter<ChooseModel> for Root {}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    use crate::theme;
    let label = label.into();
    // Panels mark the current choice with a leading check (see `workspace::mark`); draw it
    // as a selected chip.
    let selected = label.strip_prefix("✓ ").map(|s| SharedString::from(s.to_string()));
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(6.))
        .px_2()
        .py_1()
        .text_size(px(13.))
        .text_color(rgb(theme::secondary_foreground()))
        .hover(|s| s.bg(rgb(theme::secondary())).text_color(rgb(theme::foreground())))
        .focus_visible(|s| s.bg(rgb(theme::accent())).text_color(rgb(theme::foreground())))
        .when(selected.is_some(), |d| {
            d.bg(theme::alpha(theme::primary(), 0.18))
                .text_color(rgb(theme::foreground()))
                .child(theme::icon(theme::Icon::Check, 12.).text_color(rgb(theme::primary_hover())))
        })
        .child(selected.unwrap_or(label))
        .on_click(click)
}

/// Splits the session line ("provider · model · effort", or "provider · model · N% context ·
/// T tokens left (source)") into the model and, when known, the context use and tokens left.
fn session_parts(label: &str) -> (String, Option<(String, String)>) {
    let parts: Vec<&str> = label.split('·').map(str::trim).filter(|p| !p.is_empty()).collect();
    let model = parts.get(1).or(parts.first()).map_or("Model".to_string(), |m| m.to_string());
    let percent = parts.iter().find_map(|p| p.strip_suffix(" context"));
    let left = parts.iter().find_map(|p| p.split_once(" tokens left").map(|(n, _)| n));
    let context = percent.zip(left).map(|(percent, left)| {
        let left = match left.parse::<u64>() {
            Ok(n) if n >= 1000 => format!("{}K", n / 1000),
            _ => left.to_string(),
        };
        (percent.to_string(), left)
    });
    (model, context)
}

fn recorded(label: &str, status: &str, title: String, body: Doc) -> Msg {
    Msg::trace(
        Trace {
            kind: TraceKind::Recorded {
                label: label.into(),
                status: status.into(),
            },
            title,
            open: false,
        },
        body,
    )
}

fn append_saved_rows(
    messages: &[StoredMessage],
    rows: &mut Vec<Msg>,
    text_rows: &mut HashMap<String, usize>,
) {
    for message in messages {
        match message {
            StoredMessage::User { id, text, .. } => {
                text_rows.insert(id.clone(), rows.len());
                rows.push(Msg::user(text, 0));
            }
            StoredMessage::Assistant {
                id,
                text,
                thinking,
                tools,
                provider_activity,
                steps,
                error,
                ..
            } => {
                let think = |s: &str| recorded("Thought", "", String::new(), Doc::plain(s));
                let tool = |t: &zest_core::ThreadToolPart| {
                    let summary = t.summary.as_deref().unwrap_or_default();
                    let mut row = recorded(
                        &t.name,
                        &t.status,
                        first_line(summary),
                        tool_body(summary, t.diff.as_deref()),
                    );
                    row.artifacts.extend(crate::artifact::metadata_document(t.metadata.as_ref()));
                    row
                };
                let activity = |a: &zest_core::thread::ActivityPart| {
                    recorded("Provider", &a.status, a.title.clone(), Doc::default())
                };
                if steps.is_empty() {
                    if !thinking.is_empty() {
                        rows.push(think(thinking));
                    }
                    rows.extend(tools.iter().map(tool));
                    rows.extend(provider_activity.iter().map(activity));
                } else {
                    let tools_by_id: HashMap<_, _> =
                        tools.iter().rev().map(|t| (t.id.as_str(), t)).collect();
                    let activity_by_id: HashMap<_, _> = provider_activity
                        .iter()
                        .rev()
                        .map(|a| (a.id.as_str(), a))
                        .collect();
                    for step in steps {
                        match step {
                            AssistantStep::Thinking { text } => rows.push(think(text)),
                            AssistantStep::Tool { id } => {
                                rows.extend(tools_by_id.get(id.as_str()).copied().map(tool))
                            }
                            AssistantStep::Activity { id } => {
                                rows.extend(activity_by_id.get(id.as_str()).copied().map(activity))
                            }
                        }
                    }
                }
                if !text.is_empty() {
                    text_rows.insert(id.clone(), rows.len());
                    let mut row = Msg::assistant(md::parse(text));
                    row.artifacts = crate::artifact::fenced_documents(text);
                    rows.push(row);
                }
                if let Some(message) = error {
                    rows.push(Msg::assistant(Doc::plain(message)));
                }
            }
        }
    }
}

impl Root {
    pub fn open_chat(chat: native::Chat, window: &Window, cx: &mut Context<Self>) -> Self {
        let args = Args {
            a11y: true,
            ..Default::default()
        };
        let mut view = Self::new(args, chat.bridge(), window.window_handle(), cx);
        view.session_label = chat.options.lock().unwrap().provider.clone();
        view.native = Some(chat);
        view.restore_native();
        view.refresh_git_stats(cx);
        view
    }

    pub fn restore_native(&mut self) {
        let chat = self.native.as_ref().unwrap();
        let t = chat.thread.lock().unwrap();
        let (row, message) = self.native_start.take().unwrap_or((0, 0));
        let old_len = self.msgs.len();
        self.msgs.truncate(row);
        if row == 0 {
            self.native_text_rows.clear();
        }
        append_saved_rows(
            &t.messages[message..],
            &mut self.msgs,
            &mut self.native_text_rows,
        );
        self.list.splice(row..old_len, self.msgs.len() - row);
        self.pending.clear();
        for input in t
            .pending_inputs
            .iter()
            .filter(|p| p.target == ThreadInputTarget::Followup)
        {
            self.pending.push_back(QueuedPrompt {
                id: self.next_prompt_id,
                saved_id: Some(input.id.clone()),
                attachments: input.attachments.len(),
                text: input.text.clone(),
                images: input
                    .attachments
                    .iter()
                    .filter(|a| a.kind.as_deref() == Some("image"))
                    .count(),
            });
            self.next_prompt_id += 1;
        }
        self.live = None;
        self.think = None;
        self.tools.clear();
    }

    pub fn reveal_message(&self, id: &str) -> bool {
        let index = self.native_text_rows.get(id).copied().or_else(|| {
            // A streamed turn has provisional rows until the terminal save.
            let (user_row, first) = self.native_start?;
            let thread = self.native.as_ref()?.thread.lock().ok()?;
            let message = thread
                .messages
                .get(first..)?
                .iter()
                .find(|msg| msg.id() == id)?;
            match message {
                StoredMessage::User { .. } => Some(user_row),
                StoredMessage::Assistant { .. } => self.live,
            }
        });
        let Some(row) = index.filter(|row| *row < self.msgs.len()) else {
            return false;
        };
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.scroll_to_reveal_item(row);
        true
    }

    pub fn update_activity(&mut self, id: String, title: String, status: String) {
        let key = format!("provider/{id}");
        let row = recorded("Provider", &status, title, Doc::default());
        if let Some((_, ix, _)) = self.tools.iter().find(|(k, _, _)| k == &key) {
            self.msgs[*ix] = row;
            self.list.remeasure_items(*ix..*ix + 1);
        } else {
            self.finish_think();
            let ix = self.push(row);
            self.tools.push((key, ix, Instant::now()));
        }
    }

    fn accept_submission(&mut self, text: &str, images: &[Arc<Image>]) -> zest_core::Result<()> {
        let mut files = self.files.clone();
        for (i, img) in images.iter().enumerate() {
            let media = match img.format {
                ImageFormat::Png => "image/png",
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::Webp => "image/webp",
                ImageFormat::Gif => "image/gif",
                _ => {
                    return Err(zest_core::HarnessError::Other(
                        "Unsupported clipboard image format".into(),
                    ));
                }
            };
            let file = attachments::prepare_image_bytes(
                img.bytes(),
                media,
                &format!("Pasted image {}", i + 1),
            );
            if file.status != "done" {
                return Err(zest_core::HarnessError::Other(file.detail));
            }
            files.push(native::prepared(file));
        }
        let encoded: Vec<_> = files
            .iter()
            .filter_map(|f| f.data_base64.as_ref())
            .collect();
        if encoded.len() > attachments::MAX_IMAGES
            || encoded
                .iter()
                .map(|data| {
                    data.len() / 4 * 3 - data.bytes().rev().take_while(|c| *c == b'=').count()
                })
                .sum::<usize>()
                > attachments::MAX_TOTAL_IMAGE_BYTES
        {
            return Err(zest_core::HarnessError::Other(
                "A message can include up to 8 images totaling 16 MiB".into(),
            ));
        }
        let count = encoded.len();
        let input = self.native.as_ref().unwrap().enqueue(text, files)?;
        let start = !self.turn_active && self.pending.is_empty();
        self.pending.push_back(QueuedPrompt {
            id: self.next_prompt_id,
            saved_id: Some(input.id),
            attachments: input.attachments.len(),
            text: input.text,
            images: count,
        });
        self.next_prompt_id += 1;
        self.files.clear();
        self.note.clear();
        if start {
            self.start_next();
        }
        Ok(())
    }

    pub fn submit_native(&mut self, text: &str, images: &[Arc<Image>], cx: &mut Context<Self>) {
        if text.trim().eq_ignore_ascii_case("/model") {
            self.composer.update(cx, |c, cx| {
                c.images = images.to_vec();
                cx.notify();
            });
            cx.emit(ChooseModel);
            return;
        }
        if let Some(question) = zest_core::commands::btw_question(text) {
            self.composer.update(cx, |c, cx| {
                c.images = images.to_vec();
                cx.notify();
            });
            self.ask_side(question, cx);
            return;
        }
        if let Err(e) = self.accept_submission(text, images) {
            self.note = e.to_string();
            self.composer.update(cx, |c, cx| {
                c.text = text.into();
                c.sel = text.len()..text.len();
                c.images = images.to_vec();
                cx.notify();
            });
        }
        self.composer
            .update(cx, |c, _| c.has_files = !self.files.is_empty());
        cx.emit(ChatChanged);
        cx.notify();
    }

    pub fn respond(&mut self, response: Response, cx: &mut Context<Self>) {
        if let Some(card) = self.cards.pop_front() {
            let _ = card.reply.send(response);
        }
        self.decision_input.update(cx, |c, cx| {
            c.text.clear();
            c.sel = 0..0;
            c.marked = None;
            c.images.clear();
            cx.notify();
        });
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let files = cx
                .background_executor()
                .spawn(async move { attachments::prepare_paths(&paths) })
                .await;
            let _ = this.update(cx, |root, cx| {
                for f in files {
                    if f.status == "done" {
                        root.files.push(native::prepared(f));
                    } else {
                        root.note = format!("{}: {}", f.name, f.detail);
                    }
                }
                root.composer.update(cx, |c, cx| {
                    c.has_files = !root.files.is_empty();
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub fn native_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::{self, Icon};
        use crate::ui::{self, Variant};
        let mut panel = div().flex().flex_col().gap_2();
        if let Some(card) = self.cards.front() {
            let mut box_ = ui::card().p_3().flex().flex_col().gap_2().text_size(px(13.));
            match &card.request {
                Request::Approval { title, preview } => {
                    box_ = box_
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(theme::icon(Icon::ShieldCheck, 14.).text_color(rgb(theme::WARNING)))
                                .child(title.clone()),
                        )
                        .child(
                            div()
                                .id("approval-preview")
                                .max_h(px(210.))
                                .overflow_y_scroll()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(rgb(theme::border()))
                                .bg(rgb(crate::theme::input_bg()))
                                .p(px(10.))
                                .font_family(MONO)
                                .text_size(px(12.))
                                .text_color(rgb(theme::secondary_foreground()))
                                .child(preview.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(ui::button("allow-once", Variant::Primary, Some(Icon::Check), "Allow once", cx.listener(|r, _, _, cx| r.respond(Response::Allow, cx))))
                                .child(ui::button("deny", Variant::Ghost, Some(Icon::X), "Deny", cx.listener(|r, _, _, cx| r.respond(Response::Deny, cx)))),
                        );
                }
                Request::Question {
                    prompt,
                    choices,
                    multiple,
                } => {
                    box_ = box_.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(theme::icon(Icon::CircleQuestionMark, 14.).text_color(rgb(theme::link())))
                            .child(prompt.clone()),
                    );
                    let mut buttons = div().flex().flex_wrap().gap(px(6.));
                    for (index, choice) in choices.iter().enumerate() {
                        let choice = choice.clone();
                        let multi = *multiple;
                        let picked = card.selected.contains(&choice);
                        let label = if picked { format!("✓ {choice}") } else { choice.clone() };
                        buttons = buttons.child(ui::button(
                            ("choice", index),
                            if picked { Variant::Secondary } else { Variant::Outline },
                            None,
                            label,
                            cx.listener(move |r, _, _, cx| {
                                if multi {
                                    let picks = &mut r.cards.front_mut().unwrap().selected;
                                    if picks.contains(&choice) {
                                        picks.retain(|s| s != &choice);
                                    } else {
                                        picks.push(choice.clone());
                                    }
                                    cx.notify();
                                } else {
                                    r.respond(Response::Answer(vec![choice.clone()]), cx);
                                }
                            }),
                        ));
                    }
                    if *multiple && !card.selected.is_empty() {
                        buttons = buttons.child(ui::button("answer", Variant::Primary, None, "Submit choices", cx.listener(|r, _, _, cx| {
                            let answers = r.cards.front().unwrap().selected.clone();
                            r.respond(Response::Answer(answers), cx);
                        })));
                    }
                    box_ = box_
                        .child(buttons)
                        .child(self.decision_input.clone())
                        .child(div().flex().child(ui::button("dismiss-question", Variant::Ghost, None, "Dismiss", cx.listener(|r, _, _, cx| r.respond(Response::Deny, cx))).aria_label("Dismiss question")));
                }
            }
            panel = panel.child(box_);
        }
        if !self.note.is_empty() {
            panel = panel.child(
                ui::card()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .child(theme::icon(Icon::CircleAlert, 14.).text_color(rgb(theme::WARNING)))
                    .child(div().flex_1().min_w_0().child(self.note.clone()))
                    .child(ui::icon_button_xs("dismiss-notice", Icon::X, "Dismiss", cx.listener(|r, _, _, cx| {
                        r.note.clear();
                        cx.notify();
                    }))),
            );
        }
        panel
    }

    /// Recounts changed lines since the chat's start commit in the background.
    pub fn refresh_git_stats(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = &self.native else { return };
        let path = chat.project.path.clone();
        let start = chat
            .thread
            .lock()
            .unwrap()
            .git_context
            .as_ref()
            .and_then(|g| g.start_commit.clone());
        let task = crate::native::executor().spawn(async move {
            let stats = zest_core::workspace_changes::diff_stats(&path, start.as_deref()).await;
            let branch = tokio::task::spawn_blocking(move || {
                zest_core::worktree_chat::chat_git_baseline(&path).and_then(|(_, branch)| branch)
            })
            .await
            .ok()
            .flatten();
            (stats, branch)
        });
        cx.spawn(async move |this, cx| {
            if let Ok((stats, branch)) = task.await {
                let _ = this.update(cx, |root, cx| {
                    root.git_stats = Some(stats);
                    root.checkout_branch = branch;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The card above the composer with the project, branch, changed lines and linked pull
    /// request (the branch strip above the composer).
    pub fn branch_strip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        use crate::theme::{self, Icon, alpha};
        let chat = self.native.as_ref()?;
        // Shown only with changed lines and not during a turn; a dismissal
        // lasts until the counts move.
        let stats = self.git_stats.filter(|s| s.changed_files > 0 && s.additions + s.deletions > 0)?;
        if self.turn_active || self.strip_hidden == Some(stats) {
            return None;
        }
        let (branch, pr) = {
            let thread = chat.thread.lock().unwrap();
            let git = thread.git_context.as_ref();
            (
                git.and_then(|g| g.branch.clone()).or_else(|| self.checkout_branch.clone()),
                git.and_then(|g| g.pull_request.as_ref().map(|p| p.number)),
            )
        };
        let name = chat.project.path.file_name().unwrap_or_default().to_string_lossy().to_string();
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py(px(6.))
                .pl(px(10.))
                .pr(px(6.))
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(theme::soft_border()))
                .bg(rgb(theme::card()))
                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(name))
                .when_some(branch, |d, branch| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(theme::muted_foreground()))
                            .child(theme::icon(Icon::GitBranch, 12.).opacity(0.7))
                            .child(div().truncate().child(branch)),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(6.))
                        .bg(alpha(theme::secondary(), 0.7))
                        .text_size(px(11.))
                        .child(div().text_color(rgb(theme::primary())).child(format!("+{}", stats.additions)))
                        .child(div().text_color(rgb(theme::destructive())).child(format!("−{}", stats.deletions))),
                )
                .when_some(pr, |d, number| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py(px(2.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(theme::soft_border()))
                            .text_size(px(11.))
                            .text_color(rgb(theme::muted_foreground()))
                            .child(theme::icon(Icon::GitPullRequest, 12.).opacity(0.8))
                            .child(format!("#{number}")),
                    )
                })
                .child(crate::ui::icon_button("hide-branch-strip", Icon::X, "Hide branch bar", cx.listener(|root, _, _, cx| {
                    root.strip_hidden = root.git_stats;
                    cx.notify();
                }))),
        )
    }

    /// The row under the composer's text: attach, attached files, model, send or stop.
    pub fn composer_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::{self, Icon, alpha};
        use crate::ui;
        let busy = self.turn_active;
        let empty = {
            let c = self.composer.read(cx);
            c.text.trim().is_empty() && c.images.is_empty() && self.files.is_empty()
        };
        let mut left = div().flex().flex_1().min_w_0().flex_wrap().items_center().gap_1().child(ui::icon_button(
            "pick-files",
            Icon::Plus,
            "Attach files",
            cx.listener(|r, _, _, cx| r.pick_files(cx)),
        ));
        for (i, file) in self.files.iter().enumerate() {
            left = left.child(
                div()
                    .id(("file", i))
                    .role(Role::Button)
                    .aria_label(SharedString::from(format!("Remove {}", file.name)))
                    .tab_index(0)
                    .cursor_pointer()
                    .child(ui::pill(Some(Icon::Paperclip), format!("{} ×", file.name)))
                    .on_click(cx.listener(move |r, _, _, cx| {
                        if i < r.files.len() {
                            r.files.remove(i);
                        }
                        r.composer.update(cx, |c, cx| {
                            c.has_files = !r.files.is_empty();
                            cx.notify();
                        });
                        cx.notify();
                    })),
            );
        }
        let model = session_parts(&self.session_label).0;
        left = left.child(
            div()
                .id("choose-model")
                .role(Role::Button)
                .aria_label(SharedString::from(format!("Model: {model}")))
                .tab_index(0)
                .cursor_pointer()
                .min_h(px(28.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(crate::theme::model_text()))
                .hover(|s| s.bg(alpha(theme::secondary(), 0.5)))
                .child(theme::icon(Icon::Sparkles, 14.).text_color(rgb(theme::secondary_foreground())))
                .child(div().max_w(px(220.)).truncate().child(model))
                .child(theme::icon(Icon::ChevronDown, 12.).opacity(0.6))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ChooseModel))),
        );
        let send = div()
            .id(if busy { "stop-turn" } else { "send" })
            .role(Role::Button)
            .aria_label(if busy { "Stop response" } else { "Send" })
            .tab_index(0)
            .cursor_pointer()
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(rgb(theme::primary()))
            .hover(|s| s.bg(alpha(theme::primary(), 0.8)))
            .when(!busy && empty, |d| d.opacity(0.5))
            .child(theme::icon(if busy { Icon::Square } else { Icon::ArrowUp }, 14.).text_color(rgb(crate::theme::ON_PRIMARY)))
            .on_click(cx.listener(move |r, _, w, cx| {
                if r.turn_active {
                    r.stop(cx);
                } else {
                    r.composer.update(cx, |c, cx| c.send(w, cx));
                }
                w.focus(&r.composer.read(cx).focus.clone(), cx);
            }));
        div()
            .px(px(10.))
            .pb(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(left)
            .child(send)
    }

    /// The muted line under the composer card: project, branch, model and approval mode.
    pub fn status_row(&self) -> impl IntoElement {
        use crate::theme::{self, Icon};
        let mut left = div().flex().items_center().gap_2().min_w_0();
        let mut right = div().flex().items_center().gap_1();
        if let Some(chat) = &self.native {
            let name = chat.project.path.file_name().unwrap_or_default().to_string_lossy().to_string();
            left = left.child(
                div().flex().items_center().gap_1().child(theme::icon(Icon::FolderOpen, 12.).opacity(0.7)).child(name),
            );
            let thread = chat.thread.lock().unwrap();
            let branch = thread.git_context.as_ref().and_then(|g| g.branch.clone()).or_else(|| self.checkout_branch.clone());
            if let Some(branch) = branch {
                left = left.child(div().flex().items_center().gap_1().child(theme::icon(Icon::GitBranch, 12.)).child(branch));
            }
            if let Some(git) = &thread.git_context {
                if let Some(pr) = &git.pull_request {
                    left = left.child(
                        div().flex().items_center().gap_1().child(theme::icon(Icon::GitPullRequest, 12.)).child(format!("#{}", pr.number)),
                    );
                }
            }
            drop(thread);
            let mode = chat.policy.lock().unwrap().mode();
            let (label, color) = match mode {
                zest_core::ApprovalMode::Manual => ("Ask".to_string(), theme::muted_foreground()),
                zest_core::ApprovalMode::AcceptEdits => ("Edits".to_string(), theme::muted_foreground()),
                zest_core::ApprovalMode::Plan => ("Plan".to_string(), theme::link()),
                other => (format!("{other:?}"), theme::muted_foreground()),
            };
            right = right.child(
                div()
                    .min_h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(px(6.))
                    .text_color(rgb(color))
                    .child(theme::icon(Icon::Shield, 12.).text_color(rgb(color)))
                    .child(label),
            );
        }
        let (model, context) = session_parts(&self.session_label);
        if !self.session_label.is_empty() {
            left = left.child(format!("· {model}"));
        }
        if let Some((percent, remaining)) = context {
            right = right.child(
                div()
                    .min_h(px(24.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(theme::icon(Icon::CircleDot, 12.).text_color(rgb(theme::primary())))
                    .child(format!("{percent} · {remaining} left")),
            );
        }
        div()
            .mt_2()
            .px_1()
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(11.))
            .text_color(rgb(crate::theme::muted_foreground()))
            .child(left)
            .child(right)
    }
}
