use super::*;
use crate::interaction::{Request, Response};
use std::{collections::HashMap, sync::Arc};
use zest_core::{StoredMessage, ThreadInputTarget, thread::AssistantStep};

pub struct ChatChanged;

#[cfg(test)]
mod tests {
    use super::{HashMap, StoredMessage, append_saved_rows};
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
    div()
        .id(id)
        .role(Role::Button)
        .tab_index(0)
        .cursor_pointer()
        .rounded_md()
        .px_2()
        .py_1()
        .text_size(px(13.))
        .hover(|s| s.bg(rgb(0x303641)))
        .focus_visible(|s| s.bg(rgb(0x36465a)))
        .child(label.into())
        .on_click(click)
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
                    recorded(
                        &t.name,
                        &t.status,
                        first_line(summary),
                        tool_body(summary, t.diff.as_deref()),
                    )
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
                    rows.push(Msg::assistant(md::parse(text)));
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
        let mut panel = div().px_4().flex().flex_col().gap_1();
        if let Some(card) = self.cards.front() {
            let mut box_ = div()
                .rounded_md()
                .bg(rgb(PANEL))
                .border_1()
                .border_color(rgb(BORDER))
                .p_2();
            match &card.request {
                Request::Approval { title, preview } => {
                    box_ = box_
                        .child(title.clone())
                        .child(
                            div()
                                .id("approval-preview")
                                .max_h(px(210.))
                                .overflow_y_scroll()
                                .font_family(MONO)
                                .text_size(px(12.))
                                .child(preview.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(button(
                                    "allow-once",
                                    "Allow once",
                                    cx.listener(|r, _, _, cx| r.respond(Response::Allow, cx)),
                                ))
                                .child(button(
                                    "deny",
                                    "Deny",
                                    cx.listener(|r, _, _, cx| r.respond(Response::Deny, cx)),
                                )),
                        );
                }
                Request::Question {
                    prompt,
                    choices,
                    multiple,
                } => {
                    box_ = box_.child(prompt.clone());
                    let mut buttons = div().flex().flex_wrap().gap_1();
                    for (index, choice) in choices.iter().enumerate() {
                        let choice = choice.clone();
                        let multi = *multiple;
                        let label = if card.selected.contains(&choice) {
                            format!("✓ {choice}")
                        } else {
                            choice.clone()
                        };
                        buttons = buttons.child(button(
                            ("choice", index),
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
                        buttons = buttons.child(button(
                            "answer",
                            "Submit choices",
                            cx.listener(|r, _, _, cx| {
                                let answers = r.cards.front().unwrap().selected.clone();
                                r.respond(Response::Answer(answers), cx);
                            }),
                        ));
                    }
                    box_ = box_
                        .child(buttons)
                        .child(self.decision_input.clone())
                        .child(button(
                            "dismiss-question",
                            "Dismiss",
                            cx.listener(|r, _, _, cx| r.respond(Response::Deny, cx)),
                        ));
                }
            }
            panel = panel.child(box_);
        }
        if !self.note.is_empty() {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(self.note.clone()))
                    .child(button(
                        "dismiss-notice",
                        "Dismiss",
                        cx.listener(|r, _, _, cx| {
                            r.note.clear();
                            cx.notify();
                        }),
                    )),
            );
        }
        let mut bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(button(
                "pick-files",
                "Attach files",
                cx.listener(|r, _, _, cx| r.pick_files(cx)),
            ));
        for (i, file) in self.files.iter().enumerate() {
            bar = bar.child(button(
                ("file", i),
                format!("{} ×", file.name),
                cx.listener(move |r, _, _, cx| {
                    if i < r.files.len() {
                        r.files.remove(i);
                    }
                    r.composer.update(cx, |c, cx| {
                        c.has_files = !r.files.is_empty();
                        cx.notify();
                    });
                    cx.notify();
                }),
            ));
        }
        panel.child(
            bar.child(
                div()
                    .text_color(rgb(MUTED))
                    .text_size(px(12.))
                    .child(self.session_label.clone()),
            ),
        )
    }
}
