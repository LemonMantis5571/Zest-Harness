use super::*;

/// The seam between turn lifecycle decisions and the UI transport.
pub(crate) trait EventSink: Send + Sync {
    fn emit(&self, event: &ChatEvent);
}

struct TauriEventSink {
    app: AppHandle,
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &ChatEvent) {
        let _ = self.app.emit("chat-event", event);
    }
}

/// Run one complete active turn while keeping Tauri outside the lifecycle
/// implementation. The app handle is optional for tests; delegation is the
/// only part of the operation that needs it in addition to event emission.
pub(crate) async fn run(
    app: AppHandle,
    state: &AppState,
    text: String,
    attachments: Option<Vec<AttachmentInput>>,
) -> Result<(), String> {
    let thread_id = state
        .sessions
        .active_thread_id()
        .map_err(map_session_err)?
        .ok_or_else(|| desktop_err("no_session", "no active chat"))?;
    run_loop(app, state, thread_id, Some((text, attachments, None))).await
}

/// Resume the oldest durable followup only after the user explicitly asks.
/// The normal turn loop owns all subsequent FIFO followups.
pub(crate) async fn resume_queued(
    app: AppHandle,
    state: &AppState,
    thread_id: String,
) -> Result<(), String> {
    let active_thread = state
        .sessions
        .active_thread_id()
        .map_err(map_session_err)?
        .ok_or_else(|| desktop_err("no_session", "no active chat"))?;
    if active_thread != thread_id {
        return Err(desktop_err(
            "invalid",
            "queued messages can only resume in the active chat",
        ));
    }
    if state
        .sessions
        .active_turn_for_thread(&thread_id)
        .map_err(map_session_err)?
        .is_some()
    {
        return Err(desktop_err("busy", "this chat is still working"));
    }
    let input = state
        .sessions
        .peek_followup_for_thread(&thread_id)
        .map_err(map_session_err)?
        .ok_or_else(|| desktop_err("not_found", "no resumable queued messages"))?;
    let attachments = input
        .attachments
        .into_iter()
        .map(|attachment| AttachmentInput {
            name: attachment.name,
            detail: attachment.detail,
            content: attachment.content,
            status: attachment.status,
            kind: attachment.kind,
            media_type: attachment.media_type,
            data_base64: attachment.data_base64,
        })
        .collect();
    run_loop(
        app,
        state,
        thread_id,
        Some((input.text, Some(attachments), Some(input.id))),
    )
    .await
}

async fn run_loop(
    app: AppHandle,
    state: &AppState,
    thread_id: String,
    mut next: Option<(String, Option<Vec<AttachmentInput>>, Option<String>)>,
) -> Result<(), String> {
    let sink = TauriEventSink { app: app.clone() };
    while let Some((text, attachments, claim_input_id)) = next.take() {
        let completed = run_with_sink_internal(
            &sink,
            Some(app.clone()),
            state,
            text,
            attachments,
            claim_input_id,
        )
        .await?;
        if !completed {
            break;
        }
        let Some((input, root, snapshot)) = state
            .sessions
            .take_followup_for_thread(&thread_id)
            .map_err(map_session_err)?
        else {
            break;
        };
        let store = ThreadStore::open(&root).map_err(|error| error.to_string())?;
        store.save(&snapshot).map_err(|error| error.to_string())?;
        let session_id = state
            .sessions
            .session_info_snapshot(|session| session.session_id.clone())
            .map_err(map_session_err)?
            .unwrap_or_default();
        sink.emit(&ChatEvent::InputRemoved {
            session_id,
            thread_id: thread_id.clone(),
            turn_id: None,
            input_id: input.id.clone(),
        });
        next = Some((
            input.text,
            Some(
                input
                    .attachments
                    .into_iter()
                    .map(|attachment| AttachmentInput {
                        name: attachment.name,
                        detail: attachment.detail,
                        content: attachment.content,
                        status: attachment.status,
                        kind: attachment.kind,
                        media_type: attachment.media_type,
                        data_base64: attachment.data_base64,
                    })
                    .collect(),
            ),
            None,
        ));
    }
    Ok(())
}

async fn run_with_sink_internal<S: EventSink>(
    sink: &S,
    delegation_app: Option<AppHandle>,
    state: &AppState,
    text: String,
    attachments: Option<Vec<AttachmentInput>>,
    claim_input_id: Option<String>,
) -> Result<bool, String> {
    let mut text = text.trim().to_string();
    let mut attachments = attachments.unwrap_or_default();
    if text.is_empty() && !has_usable_attachment(&attachments) {
        return Err(desktop_err("invalid", "empty message"));
    }

    // The transcript keeps what was typed; only the model sees an expansion.
    let display_text = format_display_message(&text, &attachments);
    if build_user_content(&text, &attachments).is_empty() {
        return Err(desktop_err("invalid", "empty message"));
    }
    let multimodal = has_images(&attachments);

    let (mut session, turn) = state.sessions.begin_turn().map_err(map_session_err)?;
    let live_thread = turn.live_thread.clone();
    if let Some(input_id) = claim_input_id {
        let claimed = live_thread
            .lock()
            .map_err(|_| desktop_err("queue", "thread state is unavailable"))?
            .claim_input(&input_id);
        let Some(claimed) = claimed else {
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(desktop_err(
                "not_found",
                "queued message was claimed by another turn",
            ));
        };
        if claimed.target != zest_core::ThreadInputTarget::Followup {
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(desktop_err(
                "invalid",
                "only followup messages can resume an idle chat",
            ));
        }
        text = claimed.text;
        attachments = claimed
            .attachments
            .into_iter()
            .map(|attachment| AttachmentInput {
                name: attachment.name,
                detail: attachment.detail,
                content: attachment.content,
                status: attachment.status,
                kind: attachment.kind,
                media_type: attachment.media_type,
                data_base64: attachment.data_base64,
            })
            .collect();
        let worker = ensure_persist(state, &session.root)?;
        if worker
            .save_and_wait(
                PersistSnapshot::Live(live_thread.clone()),
                PersistPriority::Immediate,
            )
            .await
            .is_err()
        {
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(desktop_err(
                "persistence",
                "queued message could not be claimed safely",
            ));
        }
        sink.emit(&ChatEvent::InputRemoved {
            session_id: turn.session_id.clone(),
            thread_id: turn.thread_id.clone(),
            turn_id: Some(turn.turn_id.clone()),
            input_id,
        });
    }
    // A new user submission supersedes the one-time retry affordance restored
    // from a previous process. The durable run record remains available for
    // diagnostics, but the session no longer advertises the stale action.
    session.recovery = None;
    turn.approval_hub.begin_turn(&turn.turn_id);
    turn.question_hub.begin_turn(&turn.turn_id);

    // Capture the effective patch before the transcript or provider can touch
    // the workspace. A dirty workspace on load is deliberately harmless: only
    // a different terminal identity will produce WorkspaceChanged.
    let baseline_changes = workspace_changes_for(&session).await;
    let user_message_id = new_id("user");
    let checkpoint_preview = checkpoint_preview(&display_text);

    // Every submitted turn gets a durable checkpoint. The first one is the
    // conversation-start anchor; later ones point at the exact user message
    // that is about to be appended.
    let store = match open_store(&session.root) {
        Ok(store) => store,
        Err(error) => {
            turn.approval_hub.clear();
            turn.question_hub.clear();
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(error);
        }
    };
    let label = if live_thread
        .lock()
        .map(|thread| thread.messages.is_empty())
        .unwrap_or(false)
    {
        "Conversation start"
    } else {
        "Before turn"
    };
    let checkpoint_result = live_thread
        .lock()
        .map_err(|_| zest_core::HarnessError::Other("thread state is unavailable".into()))
        .and_then(|mut thread| {
            store.create_checkpoint_with_metadata(
                &mut thread,
                label,
                Some(user_message_id.clone()),
                Some(checkpoint_preview),
                zest_core::ThreadCheckpointKind::Turn,
            )
        });
    if let Err(error) = checkpoint_result {
        turn.approval_hub.clear();
        turn.question_hub.clear();
        let _ = state.sessions.finish_turn(&turn, session);
        return Err(error.to_string());
    }

    // Plan mode and the `plan` skill are one feature, not two things that share
    // a word: being in the mode runs the skill. A poisoned policy lock reads as
    // "not plan mode" — the tool layer fails closed on its own, and losing the
    // skill is better than losing the turn.
    let plan_mode = state
        .policy
        .lock()
        .map(|policy| policy.mode() == ApprovalMode::Plan)
        .unwrap_or(false);

    // Slash commands resolve against the session's skills, so this has to come
    // after the session is in hand. An unknown command expands to itself.
    let (prompt, command) = match session.skills.read() {
        Ok(skills) => {
            let typed = zest_core::expand_command(&text, &skills);
            // An explicit command outranks the mode: naming a skill is a
            // stronger signal than being in a mode that implies one.
            let expansion = if typed.command.is_none() && plan_mode {
                zest_core::expand_command_as(&text, &skills, PLAN_SKILL)
            } else {
                typed
            };
            (expansion.prompt, expansion.command)
        }
        // A poisoned lock must not lose the message — send it verbatim.
        Err(_) => (text.clone(), None),
    };
    let user_blocks = build_user_content(&prompt, &attachments);
    let worker = match ensure_persist(state, &session.root) {
        Ok(w) => w,
        Err(e) => {
            turn.approval_hub.clear();
            turn.question_hub.clear();
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(desktop_err("persistence", e));
        }
    };
    let persistence = match ChatPersistence::open(&session.root) {
        Ok(persistence) => persistence,
        Err(error) => {
            turn.approval_hub.clear();
            turn.question_hub.clear();
            let _ = state.sessions.finish_turn(&turn, session);
            return Err(desktop_err("persistence", error.to_string()));
        }
    };

    let session_id = turn.session_id.clone();
    let thread_id = turn.thread_id.clone();
    let turn_id = turn.turn_id.clone();
    let assistant_message_id = new_id("assistant");
    let run_persisted = match persistence.runs.create_or_resume_for_turn(
        &turn_id,
        &thread_id,
        &session.provider_id,
        &user_message_id,
        &assistant_message_id,
    ) {
        Ok(_) => true,
        Err(error) => {
            sink.emit(&ChatEvent::Warning {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                message: format!(
                    "Turn lifecycle could not be saved; chat will continue without recovery metadata: {error}"
                ),
            });
            false
        }
    };

    let user_event = ChatEvent::User {
        session_id: session_id.clone(),
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
        message_id: user_message_id,
        text: display_text,
    };
    if let Ok(mut thread) = live_thread.lock() {
        apply_event_to_thread(&mut thread, &user_event);
    }
    let assistant_start = ChatEvent::AssistantStart {
        session_id: session_id.clone(),
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
        message_id: assistant_message_id.clone(),
        command: command.clone(),
    };
    if let Ok(mut thread) = live_thread.lock() {
        apply_event_to_thread(&mut thread, &assistant_start);
        thread.record_event(zest_core::ThreadEventKind::TurnStarted {
            turn_id: turn_id.clone(),
        });
    }
    if worker
        .save_and_wait(
            PersistSnapshot::Live(live_thread.clone()),
            PersistPriority::Immediate,
        )
        .await
        .is_err()
    {
        sink.emit(&ChatEvent::Warning {
            session_id: session_id.clone(),
            thread_id: thread_id.clone(),
            turn_id: Some(turn_id.clone()),
            message: "Chat history could not be saved. You can continue, but this turn may not be available after restarting.".into(),
        });
    }
    sink.emit(&user_event);
    sink.emit(&assistant_start);

    let cancel = turn.cancel.clone();
    let delegation_coordinator = state.delegations.clone();
    let delegation_root = session.root.clone();

    // Claiming a steer/inject is a durable mutation before it reaches the
    // provider. If the process dies after this callback, the event ledger says
    // exactly which input was delivered and the queue cannot replay it.
    {
        let claim_thread = live_thread.clone();
        let claim_worker = worker.clone();
        turn.input_inbox
            .set_claim_observer(Arc::new(move |claimed| {
                if let Ok(mut thread) = claim_thread.lock() {
                    for input in claimed {
                        let _ = thread.claim_input(&input.id);
                    }
                }
                let _ = claim_worker.enqueue(
                    PersistSnapshot::Live(claim_thread.clone()),
                    PersistPriority::Immediate,
                );
            }));
    }

    let result = {
        // Capture external context once per turn. A song title is user-device
        // metadata, not durable conversation content, and it must not drift
        // between provider rounds or leak into the saved transcript.
        let previous_system = session.agent.system.clone();
        let plugin_context = tauri::async_runtime::spawn_blocking(plugins::agent_context)
            .await
            .ok()
            .flatten();
        if let Some(context) = plugin_context {
            // Into the volatile half, never the cached one. This is the most
            // changeable text in the whole prompt — a song title turns over
            // every few minutes — and appending it to the cached block would
            // evict the base prompt, project docs, and skills along with it,
            // every time the track changed.
            let mut system = session
                .agent
                .system
                .clone()
                .unwrap_or_else(|| SystemPrompt::new(DEFAULT_SYSTEM));
            if !system.volatile.is_empty() {
                system.volatile.push_str("\n\n");
            }
            system.volatile.push_str("# Enabled local integrations\n\n");
            system.volatile.push_str(&context);
            session.agent.system = Some(system);
        }
        let assistant_message_id = assistant_message_id.clone();
        let session_id = session_id.clone();
        let thread_id = thread_id.clone();
        let turn_id = turn_id.clone();
        let live_thread = live_thread.clone();
        let worker = worker.clone();
        let persistence = persistence.clone();
        let delegation_app = delegation_app.clone();
        // Whether a tool has run since the last thing the model said.
        //
        // A tool-using turn is several provider rounds, and all of them write
        // into one assistant message. Without this, the sentence that closes
        // one round and the sentence that opens the next are concatenated with
        // nothing between them — "…survey the current state first.Now I have
        // the full picture." — which reads as one long malformed paragraph
        // rather than as the several separate remarks it actually is.
        let mut round_break_pending = false;
        let mut on_event = move |ev: StreamEvent<'_>| {
            let ev = match ev {
                StreamEvent::ResumeHandle(handle) => {
                    if run_persisted {
                        let _ = persistence.runs.set_resume_handle(&turn_id, handle);
                    }
                    return;
                }
                other => other,
            };
            let event = match ev {
                StreamEvent::Text(t) => {
                    // Only at a real seam: the break is inserted where a tool
                    // actually ran, never between chunks of one continuous
                    // reply, and never when the model already started its own
                    // paragraph.
                    let text = if round_break_pending && !t.trim_start().is_empty() {
                        round_break_pending = false;
                        if t.starts_with('\n') {
                            t.to_string()
                        } else {
                            format!("\n\n{t}")
                        }
                    } else {
                        t.to_string()
                    };
                    ChatEvent::TextDelta {
                        session_id: session_id.clone(),
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        message_id: assistant_message_id.clone(),
                        text,
                    }
                }
                StreamEvent::Thinking(t) => ChatEvent::ThinkingDelta {
                    session_id: session_id.clone(),
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    message_id: assistant_message_id.clone(),
                    text: t.to_string(),
                },
                StreamEvent::ProviderActivity { id, title, status } => {
                    ChatEvent::ProviderActivity {
                        session_id: session_id.clone(),
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        message_id: assistant_message_id.clone(),
                        id: id.to_string(),
                        title: title.to_string(),
                        status: provider_activity_status(status).into(),
                    }
                }
                StreamEvent::ToolCallStart { name, id } => ChatEvent::ToolCallStart {
                    session_id: session_id.clone(),
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    message_id: assistant_message_id.clone(),
                    name: name.to_string(),
                    id: id.to_string(),
                },
                StreamEvent::ToolCallUpdate { name: _, id, metadata } => {
                    ChatEvent::ToolCallUpdate {
                        session_id: session_id.clone(),
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        message_id: assistant_message_id.clone(),
                        id: id.to_string(),
                        metadata: ToolMetaView::from(metadata),
                    }
                }
                StreamEvent::ToolCallResult {
                    name,
                    id,
                    summary,
                    is_error,
                    path,
                    diff,
                    metadata,
                } => {
                    let delegation_job_id = metadata.as_ref().and_then(|metadata| match metadata {
                        ToolMetadata::Delegation { job_id, .. } => job_id.clone(),
                    });
                    // Whatever the model says next belongs to a new round.
                    round_break_pending = true;
                    let event = ChatEvent::ToolCallResult {
                        session_id: session_id.clone(),
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        message_id: assistant_message_id.clone(),
                        name: name.to_string(),
                        id: id.to_string(),
                        summary: summary.to_string(),
                        is_error,
                        path: path.map(str::to_string),
                        diff: diff.map(str::to_string),
                        metadata: metadata.map(ToolMetaView::from),
                    };
                    if let (Some(app), Some(job_id)) = (delegation_app.as_ref(), delegation_job_id)
                    {
                        // `delegate_feature` already passed the model/tool approval gate;
                        // that approval is the explicit approval for this exact card.
                        let _ = delegation_coordinator.approve(
                            app,
                            &delegation_root,
                            &job_id,
                        );
                    }
                    event
                }
                StreamEvent::ApprovalNeeded {
                    approval_id,
                    tool_name,
                    tool_call_id,
                    risk,
                    path,
                    summary,
                    diff,
                } => ChatEvent::ApprovalNeeded {
                    session_id: session_id.clone(),
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    message_id: assistant_message_id.clone(),
                    approval_id,
                    tool_name,
                    tool_call_id,
                    risk: tool_risk_wire(risk).into(),
                    path,
                    summary,
                    diff,
                },
                StreamEvent::QuestionNeeded {
                    question_id,
                    tool_call_id,
                    prompt,
                    choices,
                    multiple,
                    placeholder,
                } => ChatEvent::QuestionNeeded {
                    session_id: session_id.clone(),
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    message_id: assistant_message_id.clone(),
                    question_id,
                    tool_call_id,
                    prompt,
                    choices,
                    multiple,
                    placeholder,
                },
                // Surfaced as a warning rather than swallowed: the model chip
                // shows what was *requested*, so without this the transcript
                // would silently attribute a turn to the wrong model.
                StreamEvent::ModelSubstituted { served, .. } => ChatEvent::Warning {
                    session_id: session_id.clone(),
                    thread_id: thread_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    message: format!(
                        "The selected model was unavailable, so this response used `{served}` instead."
                    ),
                },
                StreamEvent::ResumeHandle(_) => {
                    unreachable!("resume handles are persisted before chat-event mapping")
                }
            };

            if run_persisted {
                match &event {
                    ChatEvent::ApprovalNeeded {
                        approval_id,
                        tool_name,
                        tool_call_id,
                        risk,
                        path,
                        summary,
                        diff,
                        ..
                    } => {
                        let result = persistence.interrupts.create(
                            approval_id,
                            &turn_id,
                            &thread_id,
                            json!({
                                "kind": "approval",
                                "approvalId": approval_id,
                                "toolName": tool_name,
                                "toolCallId": tool_call_id,
                                "risk": risk,
                                "path": path,
                                "summary": summary,
                                "diff": diff,
                            }),
                        );
                        if result.is_ok() {
                            let _ = persistence.runs.mark_interrupted(&turn_id);
                        }
                    }
                    ChatEvent::QuestionNeeded {
                        question_id,
                        tool_call_id,
                        prompt,
                        choices,
                        multiple,
                        placeholder,
                        ..
                    } => {
                        let result = persistence.interrupts.create(
                            question_id,
                            &turn_id,
                            &thread_id,
                            json!({
                                "kind": "question",
                                "questionId": question_id,
                                "toolCallId": tool_call_id,
                                "prompt": prompt,
                                "choices": choices,
                                "multiple": multiple,
                                "placeholder": placeholder,
                            }),
                        );
                        if result.is_ok() {
                            let _ = persistence.runs.mark_interrupted(&turn_id);
                        }
                    }
                    _ => {}
                }
            }

            // The lock is released before the worker is told anything, so the
            // deferred read below can never wait on this event's own guard.
            let priority = if matches!(&event, ChatEvent::ProviderActivity { .. }) {
                None
            } else {
                match live_thread.lock() {
                    Ok(mut thread) => {
                        record_ledger_event(&mut thread, &event);
                        apply_event_to_thread(&mut thread, &event);
                        event_mutates_thread(&event).then(|| event_priority(&event))
                    }
                    Err(_) => None,
                }
            };

            if let Some(priority) = priority {
                // Immediate for tools/approvals/terminal, Delta for text and
                // thinking. Either way the worker is handed the live thread
                // rather than a copy of it: deltas arrive dozens of times a
                // second and coalesce into one write, so copying the whole
                // conversation per delta produced a transcript-sized allocation
                // for every chunk and threw nearly all of them away.
                let snapshot = PersistSnapshot::Live(live_thread.clone());
                if worker.enqueue(snapshot, priority).is_err() {
                    sink.emit(&ChatEvent::Warning {
                        session_id: session_id.clone(),
                        thread_id: thread_id.clone(),
                        turn_id: Some(turn_id.clone()),
                        message: "Chat history could not be saved. You can continue, but this turn may not be available after restarting.".into(),
                    });
                }
            }
            sink.emit(&event);
        };

        let result = if multimodal {
            session
                .agent
                .send_blocks_cancellable_with_inbox(
                    user_blocks,
                    &mut on_event,
                    Some(&cancel),
                    Some(&turn.input_inbox),
                )
                .await
        } else {
            // Text-only path keeps prior wire shape (single text block).
            let agent_text = user_blocks
                .iter()
                .find_map(|b| {
                    (b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .then(|| {
                            b.get("text")
                                .and_then(|t| t.as_str())
                                .map(|s| s.to_string())
                        })
                        .flatten()
                })
                .unwrap_or_default();
            session
                .agent
                .send_cancellable_with_inbox(
                    &agent_text,
                    &mut on_event,
                    Some(&cancel),
                    Some(&turn.input_inbox),
                )
                .await
        };
        session.agent.system = previous_system;
        result
    };

    session.thread = live_thread
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    // Wire history is already transactional inside Agent; only sync committed
    // messages after a successful terminal turn.
    let final_event = match &result {
        Ok(()) => {
            // Persist redacted wire history; live agent memory keeps secrets.
            session
                .thread
                .set_agent_messages(session.agent.messages_for_persist());
            session.thread.provider_session = session.agent.provider_session();
            ChatEvent::Done {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                message_id: assistant_message_id.clone(),
            }
        }
        Err(HarnessError::Cancelled) => {
            // Keep UI transcript; leave agent.messages at the last committed turn.
            // Terminalize any pending approval/running tool cards.
            let _ = session.thread.terminalize_interrupted();
            session
                .thread
                .set_agent_messages(session.agent.messages_for_persist());
            session.agent.clear_provider_session();
            session.thread.provider_session = None;
            ChatEvent::Cancelled {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                message_id: assistant_message_id.clone(),
            }
        }
        Err(e) => {
            let _ = session.thread.terminalize_interrupted();
            session
                .thread
                .set_agent_messages(session.agent.messages_for_persist());
            session.agent.clear_provider_session();
            session.thread.provider_session = None;
            ChatEvent::Error {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                message_id: assistant_message_id.clone(),
                message: format_turn_error_for_provider(e, &session.provider_id),
                // Only for failures that signing in again actually fixes.
                reconnect_provider: reconnect_provider_for_auth_failure(e, &session.provider_id),
                provider_selection: provider_selection_for_auth_failure(e, &session.provider_id),
            }
        }
    };
    if let Some(change) = changed_workspace(&baseline_changes, &session).await {
        sink.emit(&ChatEvent::WorkspaceChanged {
            session_id: session_id.clone(),
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            change: change.into(),
        });
    }
    record_ledger_event_with_status(&mut session.thread, &final_event, &turn_id);
    apply_event_to_thread(&mut session.thread, &final_event);
    let history_save_failed = if worker
        .save_and_wait(
            PersistSnapshot::owned(session.thread.clone()),
            PersistPriority::Immediate,
        )
        .await
        .is_err()
    {
        true
    } else {
        worker.flush().await.is_err()
    };
    if history_save_failed {
        sink.emit(&ChatEvent::Warning {
            session_id: session_id.clone(),
            thread_id: thread_id.clone(),
            turn_id: Some(turn_id.clone()),
            message: "Chat history could not be saved. You can continue, but this turn may not be available after restarting.".into(),
        });
    }
    let final_history_saved = !history_save_failed;

    // Keep the ordering invariant: the transcript is durable before a
    // successful run is marked terminal. If the transcript write failed, leave
    // the lifecycle record failed rather than claiming a completed conversation
    // that cannot be restored.
    if run_persisted {
        let lifecycle_result = if !final_history_saved {
            let _ = persistence.interrupts.cancel_pending_by_run(&turn_id);
            persistence
                .runs
                .mark_failed(&turn_id, "final transcript persistence failed")
        } else {
            match &result {
                Ok(()) => persistence
                    .runs
                    .mark_completed(&turn_id, session.agent.last_usage.as_ref()),
                Err(HarnessError::Cancelled) => {
                    let _ = persistence.interrupts.cancel_pending_by_run(&turn_id);
                    persistence.runs.mark_aborted(&turn_id)
                }
                Err(error) => {
                    let _ = persistence.interrupts.cancel_pending_by_run(&turn_id);
                    persistence.runs.mark_failed(&turn_id, error.to_string())
                }
            }
        };
        if let Err(error) = lifecycle_result {
            sink.emit(&ChatEvent::Warning {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                message: format!("Turn lifecycle could not be finalized: {error}"),
            });
        }
    }
    sink.emit(&final_event);

    turn.approval_hub.clear();
    turn.question_hub.clear();
    let _ = state.sessions.finish_turn(&turn, session);

    // Error/cancel already emitted as chat-events; keep invoke Ok to avoid
    // double toasts on the frontend catch path.
    Ok(matches!(result, Ok(())))
}

async fn workspace_changes_for(session: &Session) -> Option<zest_core::WorkspaceChangeSet> {
    let context = session.thread.git_context.as_ref();
    zest_core::workspace_changes::inspect(
        &session.root,
        context.and_then(|context| context.start_commit.as_deref()),
        context.and_then(|context| context.base_branch.as_deref()),
    )
    .await
    .ok()
}

fn record_ledger_event(thread: &mut Thread, event: &ChatEvent) {
    match event {
        ChatEvent::ToolCallStart {
            turn_id, id, name, ..
        } => {
            thread.record_event(zest_core::ThreadEventKind::ToolCalled {
                turn_id: turn_id.clone(),
                call_id: id.clone(),
                name: name.clone(),
            });
        }
        ChatEvent::ApprovalNeeded {
            turn_id,
            approval_id,
            tool_call_id,
            tool_name,
            ..
        } => {
            thread.record_event(zest_core::ThreadEventKind::ApprovalRequested {
                turn_id: turn_id.clone(),
                approval_id: approval_id.clone(),
                call_id: tool_call_id.clone(),
                name: tool_name.clone(),
            });
        }
        ChatEvent::ToolCallResult {
            turn_id,
            id,
            name,
            is_error,
            ..
        } => {
            thread.record_event(zest_core::ThreadEventKind::ToolResult {
                turn_id: turn_id.clone(),
                call_id: id.clone(),
                name: name.clone(),
                is_error: *is_error,
            });
        }
        _ => {}
    }
}

fn record_ledger_event_with_status(thread: &mut Thread, event: &ChatEvent, turn_id: &str) {
    let status = match event {
        ChatEvent::Done { .. } => "completed",
        ChatEvent::Cancelled { .. } => "cancelled",
        ChatEvent::Error { .. } => "failed",
        _ => return,
    };
    thread.record_event(zest_core::ThreadEventKind::TurnCompleted {
        turn_id: turn_id.to_string(),
        status: status.into(),
    });
}

async fn changed_workspace(
    baseline: &Option<zest_core::WorkspaceChangeSet>,
    session: &Session,
) -> Option<zest_core::WorkspaceChangeSet> {
    let latest = workspace_changes_for(session).await?;
    let baseline = baseline.as_ref()?;
    (baseline.change_id != latest.change_id).then_some(latest)
}

fn checkpoint_preview(text: &str) -> String {
    let preview = text.split_whitespace().collect::<Vec<_>>().join(" ");
    preview.chars().take(180).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<ChatEvent>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: &ChatEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn event_sink_preserves_event_order_without_tauri() {
        let sink = RecordingSink::default();
        let first = ChatEvent::Warning {
            session_id: "session-1".to_string(),
            thread_id: "thread-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            message: "first".to_string(),
        };
        let second = ChatEvent::Warning {
            session_id: "session-1".to_string(),
            thread_id: "thread-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            message: "second".to_string(),
        };

        sink.emit(&first);
        sink.emit(&second);

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ChatEvent::Warning { message, .. } if message == "first"));
        assert!(matches!(&events[1], ChatEvent::Warning { message, .. } if message == "second"));
    }

    #[test]
    fn event_sink_keeps_transcript_events_before_terminal_state() {
        let sink = RecordingSink::default();
        let session_id = "session-1".to_string();
        let thread_id = "thread-1".to_string();
        let turn_id = "turn-1".to_string();

        for event in [
            ChatEvent::User {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                message_id: "user-1".to_string(),
                text: "hello".to_string(),
            },
            ChatEvent::AssistantStart {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                message_id: "assistant-1".to_string(),
                command: None,
            },
            ChatEvent::TextDelta {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                message_id: "assistant-1".to_string(),
                text: "world".to_string(),
            },
            ChatEvent::Done {
                session_id,
                thread_id,
                turn_id,
                message_id: "assistant-1".to_string(),
            },
        ] {
            sink.emit(&event);
        }

        let events = sink.events.lock().unwrap();
        assert!(matches!(
            events.as_slice(),
            [
                ChatEvent::User { .. },
                ChatEvent::AssistantStart { .. },
                ChatEvent::TextDelta { .. },
                ChatEvent::Done { .. }
            ]
        ));
    }

    #[test]
    fn lifecycle_ledger_records_tool_approval_result_and_terminal_order() {
        let mut thread = Thread::new();
        let thread_id = thread.id.clone();
        let turn_id = "turn-ledger";
        record_ledger_event(
            &mut thread,
            &ChatEvent::ToolCallStart {
                session_id: "s".into(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.into(),
                message_id: "a".into(),
                name: "bash".into(),
                id: "call-1".into(),
            },
        );
        record_ledger_event(
            &mut thread,
            &ChatEvent::ApprovalNeeded {
                session_id: "s".into(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.into(),
                message_id: "a".into(),
                approval_id: "approval-1".into(),
                tool_name: "bash".into(),
                tool_call_id: "call-1".into(),
                risk: "exec".into(),
                path: "cargo test".into(),
                summary: "run tests".into(),
                diff: String::new(),
            },
        );
        record_ledger_event(
            &mut thread,
            &ChatEvent::ToolCallResult {
                session_id: "s".into(),
                thread_id: thread_id.clone(),
                turn_id: turn_id.into(),
                message_id: "a".into(),
                name: "bash".into(),
                id: "call-1".into(),
                summary: "passed".into(),
                is_error: false,
                path: None,
                diff: None,
                metadata: None,
            },
        );
        record_ledger_event_with_status(
            &mut thread,
            &ChatEvent::Done {
                session_id: "s".into(),
                thread_id,
                turn_id: turn_id.into(),
                message_id: "a".into(),
            },
            turn_id,
        );

        assert!(matches!(
            thread.events.iter().map(|entry| &entry.event).collect::<Vec<_>>().as_slice(),
            [
                ThreadEventKind::ToolCalled { .. },
                ThreadEventKind::ApprovalRequested { .. },
                ThreadEventKind::ToolResult { .. },
                ThreadEventKind::TurnCompleted { status, .. }
            ] if status == "completed"
        ));
        assert!(thread
            .events
            .windows(2)
            .all(|events| events[0].sequence < events[1].sequence));
    }
}
