use super::*;
use crate::native_view::button;
use std::sync::Arc;
use zest_core::{CancelToken, StreamEvent, btw::SideConversation};

#[derive(Default)]
pub struct Side {
    pub open: bool,
    busy: bool,
    generation: usize,
    conversation: Arc<tokio::sync::Mutex<Option<SideConversation>>>,
    cancel: CancelToken,
    history: Vec<(String, String)>,
    error: String,
}

impl Drop for Side {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Side {
    pub fn finished_answer(&self) -> bool {
        !self.busy
            && self
                .history
                .last()
                .is_some_and(|(_, text)| !text.is_empty())
    }
    pub fn cancel_current(&self) {
        self.cancel.cancel();
    }
}

impl Root {
    pub fn close_side(&mut self, cx: &mut Context<Self>) {
        let generation = self.side.generation + 1;
        self.side = Side::default();
        self.side.generation = generation;
        self.side_input.update(cx, |c, cx| {
            c.text.clear();
            c.sel = 0..0;
            c.marked = None;
            cx.notify();
        });
        cx.notify();
    }

    pub fn ask_side(&mut self, prompt: &str, cx: &mut Context<Self>) {
        self.side.open = true;
        if prompt.is_empty() {
            cx.notify();
            return;
        }
        if self.side.busy {
            self.side.error = "Finish or stop the current side answer first.".into();
            self.side_input.update(cx, |c, cx| {
                c.text = prompt.into();
                c.sel = prompt.len()..prompt.len();
                cx.notify();
            });
            cx.notify();
            return;
        }
        let chat = self.native.as_ref().unwrap().clone();
        let session = self.side.conversation.clone();
        self.side.cancel = CancelToken::new();
        let cancel = self.side.cancel.clone();
        self.side.generation += 1;
        let generation = self.side.generation;
        self.side.busy = true;
        self.side.error.clear();
        self.side.history.push((prompt.into(), String::new()));
        let prompt = prompt.to_owned();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
        let task = native::executor().spawn(async move {
            let mut session = session.lock().await;
            if session.is_none() {
                let (events, _unused) = futures::channel::mpsc::unbounded();
                let (mut agent, _, _) = chat.agent(&events)?;
                let t = chat.thread.lock().unwrap();
                if matches!(
                    t.messages.last(),
                    Some(zest_core::StoredMessage::Assistant {
                        streaming: true,
                        ..
                    })
                ) {
                    if let Some(zest_core::StoredMessage::User { text, .. }) =
                        t.messages.iter().rev().nth(1)
                    {
                        agent
                            .messages
                            .push(zest_core::Message::user_text(text.clone()));
                    }
                }
                *session = Some(agent.side_conversation().without_provider_session());
            }
            session
                .as_mut()
                .unwrap()
                .send(&prompt, &cancel, &mut |event: StreamEvent<'_>| {
                    if let StreamEvent::Text(t) = event {
                        let _ = tx.unbounded_send(t.into());
                    }
                })
                .await
                .map_err(native::provider_failure)
        });
        cx.spawn(async move |this, cx| {
            while let Some(text) = rx.next().await {
                let _ = this.update(cx, |r, cx| {
                    if r.side.generation == generation {
                        if let Some((_, answer)) = r.side.history.last_mut() {
                            answer.push_str(&text);
                        }
                        cx.notify();
                    }
                });
            }
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |r, cx| {
                if r.side.generation != generation {
                    return;
                }
                r.side.busy = false;
                match result {
                    Ok(text) => {
                        if let Some((_, answer)) = r.side.history.last_mut() {
                            *answer = text;
                        }
                    }
                    Err(e) => {
                        r.side.error = e;
                        if let Some((q, _)) = r.side.history.pop() {
                            r.side_input.update(cx, |c, cx| {
                                c.sel = q.len()..q.len();
                                c.text = q;
                                cx.notify();
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn render_side(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div()
            .id("side-history")
            .max_h(px(190.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for (q, a) in &self.side.history {
            body = body
                .child(div().mt_1().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(rgb(LINK)).child(q.clone()))
                .child(div().text_size(px(13.)).child(a.clone()));
        }
        crate::ui::card()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::theme::icon(crate::theme::Icon::MessageSquare, 14.))
                    .child(div().flex_1().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("Side conversation"))
                    .when(self.side.busy, |d| {
                        d.child(button(
                            "stop-side",
                            "Stop side answer",
                            cx.listener(|r, _, _, _| r.side.cancel.cancel()),
                        ))
                    })
                    .child(button(
                        "close-side",
                        "Close",
                        cx.listener(|r, _, _, cx| r.close_side(cx)),
                    )),
            )
            .child(body)
            .when(!self.side.error.is_empty(), |d| {
                d.child(self.side.error.clone())
            })
            .child(self.side_input.clone())
    }
}
