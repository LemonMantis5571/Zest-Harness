import type {
  ChatEvent,
  ChatMessage,
  ProviderActivityPart,
  ToolPart,
} from "./types.ts";

/** Pure chat UI projection state reduced from desktop `chat-event` payloads. */
export type ChatUiState = {
  messages: ChatMessage[];
  activeAssistantId: string | null;
  sending: boolean;
  /** Accept events only for this session/thread when set. */
  sessionId: string | null;
  threadId: string | null;
  /** Current in-flight turn; mismatched turn_id events are ignored. */
  currentTurnId: string | null;
};

export type ChatReduceEffects = {
  /** App shows a toast when set (reducer stays side-effect free). */
  errorToast?: string;
  warningToast?: string;
};

export type ChatReduceResult = {
  state: ChatUiState;
  effects: ChatReduceEffects;
};

export type ChatReduceOptions = {
  /** Injected for deterministic tests; production uses crypto UUIDs. */
  newId?: (prefix: string) => string;
};

function defaultNewId(prefix: string): string {
  return `${prefix}-${crypto.randomUUID()}`;
}

/**
 * Thinking chunks often arrive as separate sentences without a separator.
 * Insert a blank line when a capitalised chunk follows a finished word.
 *
 * Summarized thinking arrives as a run of `**Title**` blocks. Concatenated
 * naively, one block's closing `**` meets the next block's opening `**` and
 * fuses into `****`, which is not valid emphasis — the reader sees literal
 * asterisks welding two headings together.
 */
export function joinThinkingStream(prev: string, delta: string): string {
  if (!prev) return delta;
  if (!delta) return prev;
  // Check the marker boundary before the whitespace shortcut: the fused case
  // has no whitespace on either side, which is precisely why it fuses.
  if (/\*\*$/.test(prev) && /^\*\*/.test(delta)) {
    return `${prev}\n\n${delta}`;
  }
  // A `**` chunk arriving after a finished sentence is the next titled step,
  // not emphasis inside the old one. Only the `**`-meets-`**` case was handled,
  // so a step following prose glued into `…moving on.**Next title**`: the
  // heading rendered inline mid-paragraph instead of starting a block, and
  // nothing downstream could tell the steps apart to count or summarise them.
  //
  // Gated on sentence-ending punctuation rather than "any non-space", so
  // genuine inline emphasis (`the**gateway**`) is still left alone.
  if (/^\*\*/.test(delta) && /[.!?:]$/.test(prev)) {
    return `${prev}\n\n${delta}`;
  }
  if (/\s$/.test(prev) || /^\s/.test(delta)) return prev + delta;
  if (/[A-Za-z0-9)]$/.test(prev) && /^[A-Z]/.test(delta)) {
    return `${prev}\n\n${delta}`;
  }
  return prev + delta;
}

function eventSessionId(event: ChatEvent): string | undefined {
  return event.session_id;
}

function eventThreadId(event: ChatEvent): string | undefined {
  return event.thread_id;
}

function eventTurnId(event: ChatEvent): string | undefined {
  const turn = event.turn_id;
  return turn == null ? undefined : turn;
}

/** Drop events that belong to a different session, thread, or turn. */
export function isStaleChatEvent(state: ChatUiState, event: ChatEvent): boolean {
  const sid = eventSessionId(event);
  const tid = eventThreadId(event);
  if (state.sessionId && sid && sid !== state.sessionId) return true;
  if (state.threadId && tid && tid !== state.threadId) return true;
  const turn = eventTurnId(event);
  if (
    state.currentTurnId &&
    turn &&
    turn !== state.currentTurnId &&
    event.kind !== "warning"
  ) {
    return true;
  }
  return false;
}

/**
 * Close out tool rows that a terminal turn left waiting on a human.
 *
 * A turn cannot end with an approval still outstanding — the harness awaits the
 * decision — so anything still `awaiting_approval` when `done`/`cancelled`/
 * `error` arrives is a row whose waiter the backend has already dropped.
 *
 * Leaving them alone was a deadlock, not cosmetic. `pendingApprovals` in
 * ChatScreen scans *every* assistant message in the thread and shows
 * `pendingApprovals[0]`, so one dead row from an earlier turn sat at the head
 * of the queue and hid the live card behind it ("1 of 2"). Resolving the dead
 * row sent an id `ApprovalHub` had already dropped, which came back as
 * "no pending approval with that id" — surfaced as a generic
 * "Something went wrong" — and since the row never changed status the queue
 * could never advance. The restore-from-disk path in App.tsx has always done
 * this; the live path never did.
 */
function terminalizeInterruptedTools(tools: ToolPart[]): ToolPart[] {
  if (
    !tools.some((t) => t.status === "awaiting_approval" || t.status === "running")
  ) {
    return tools;
  }
  return tools.map((tool) => {
    if (tool.status !== "awaiting_approval" && tool.status !== "running") {
      return tool;
    }
    const note =
      tool.status === "awaiting_approval" ? "approval interrupted" : "interrupted";
    return {
      ...tool,
      status: "error" as const,
      summary: tool.summary ? `${tool.summary} (${note})` : note,
    };
  });
}

/** Apply {@link terminalizeInterruptedTools} across the whole transcript. */
function sweepInterruptedTools(state: ChatUiState): ChatUiState {
  let changed = false;
  const messages = state.messages.map((message) => {
    if (message.role !== "assistant") return message;
    const tools = terminalizeInterruptedTools(message.tools);
    if (tools === message.tools) return message;
    changed = true;
    return { ...message, tools };
  });
  return changed ? { ...state, messages } : state;
}

function ensureAssistant(
  state: ChatUiState,
  messageId: string | undefined,
  newId: (prefix: string) => string
): { state: ChatUiState; id: string } {
  if (messageId) {
    let messages = state.messages;
    const last = messages[messages.length - 1];
    const existsAtTail = last?.id === messageId;
    if (!existsAtTail && !messages.some((m) => m.id === messageId)) {
      messages = [
        ...messages,
        {
          id: messageId,
          role: "assistant",
          text: "",
          thinking: "",
          tools: [],
          streaming: true,
        },
      ];
    }
    return {
      state: { ...state, messages, activeAssistantId: messageId },
      id: messageId,
    };
  }
  if (state.activeAssistantId) {
    return { state, id: state.activeAssistantId };
  }
  const id = newId("assistant");
  return {
    state: {
      ...state,
      activeAssistantId: id,
      messages: [
        ...state.messages,
        {
          id,
          role: "assistant",
          text: "",
          thinking: "",
          tools: [],
          streaming: true,
        },
      ],
    },
    id,
  };
}

function patchAssistant(
  state: ChatUiState,
  id: string,
  patch: (msg: Extract<ChatMessage, { role: "assistant" }>) => ChatMessage
): ChatUiState {
  // Streaming updates overwhelmingly target the last transcript row. Keep
  // that hot path O(1) for lookup, while retaining the backwards search for
  // provider events that update an earlier assistant row.
  let index = state.messages.length - 1;
  const tail = state.messages[index];
  if (tail?.id !== id || tail.role !== "assistant") {
    for (index -= 1; index >= 0; index -= 1) {
      const candidate = state.messages[index];
      if (candidate.id === id && candidate.role === "assistant") break;
    }
  }
  if (index < 0) return state;

  const current = state.messages[index];
  if (current.role !== "assistant") return state;
  const next = patch(current);
  if (next === current) return state;

  const messages = state.messages.slice();
  messages[index] = next;
  return {
    ...state,
    messages,
  };
}

function providerActivityStatus(status: string): ProviderActivityPart["status"] {
  switch (status.toLowerCase()) {
    case "done":
    case "complete":
    case "completed":
    case "success":
    case "succeeded":
      return "done";
    case "error":
    case "failed":
    case "failure":
    case "cancelled":
    case "canceled":
      return "error";
    default:
      return "running";
  }
}

function finishProviderActivities(
  activities: ProviderActivityPart[] | undefined,
  status: ProviderActivityPart["status"]
): ProviderActivityPart[] | undefined {
  if (!activities?.length) return activities;
  return activities.map((activity) =>
    activity.status === "running" ? { ...activity, status } : activity
  );
}

/**
 * Characterize / apply a single chat-event to UI state.
 * Mirrors the former App.tsx `handleChatEvent` switch (no side effects).
 */
export function reduceChatEvent(
  state: ChatUiState,
  event: ChatEvent,
  options?: ChatReduceOptions
): ChatReduceResult {
  const newId = options?.newId ?? defaultNewId;
  const effects: ChatReduceEffects = {};

  if (isStaleChatEvent(state, event)) {
    return { state, effects };
  }

  switch (event.kind) {
    case "user": {
      if (state.messages.some((m) => m.id === event.message_id)) {
        return {
          state: {
            ...state,
            activeAssistantId: null,
            currentTurnId: event.turn_id,
            sending: true,
          },
          effects,
        };
      }
      return {
        state: {
          ...state,
          activeAssistantId: null,
          currentTurnId: event.turn_id,
          sending: true,
          messages: [
            ...state.messages,
            { id: event.message_id, role: "user", text: event.text },
          ],
        },
        effects,
      };
    }
    case "assistant_start": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      const withCommand = event.command
        ? patchAssistant(ensured.state, ensured.id, (m) => ({
            ...m,
            command: event.command ?? undefined,
          }))
        : ensured.state;
      return {
        state: {
          ...withCommand,
          currentTurnId: event.turn_id,
          sending: true,
          activeAssistantId: ensured.id,
        },
        effects,
      };
    }
    case "text_delta": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => ({
          ...m,
          text: m.text + event.text,
          streaming: true,
        })),
        effects,
      };
    }
    case "thinking_delta": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => ({
          ...m,
          thinking: joinThinkingStream(m.thinking, event.text),
          streaming: true,
        })),
        effects,
      };
    }
    case "provider_activity": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      const activity: ProviderActivityPart = {
        id: event.id,
        title: event.title || "Provider activity",
        status: providerActivityStatus(event.status),
      };
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => {
          const current = m.providerActivity ?? [];
          const existing = current.find((item) => item.id === activity.id);
          const next = existing
            ? current.map((item) =>
                item.id === activity.id
                  ? {
                      ...item,
                      title:
                        activity.title === "External tool"
                          ? item.title
                          : activity.title,
                      status: activity.status,
                    }
                  : item
              )
            : [...current, activity];
          return { ...m, providerActivity: next, streaming: true };
        }),
        effects,
      };
    }
    case "tool_call_start": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      const toolId = event.id;
      const tool: ToolPart = { id: toolId, name: event.name, status: "running" };
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => {
          if (m.tools.some((t) => t.id === toolId)) return m;
          return { ...m, tools: [...m.tools, tool], streaming: true };
        }),
        effects,
      };
    }
    case "tool_call_update": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => ({
          ...m,
          tools: m.tools.map((tool) =>
            tool.id === event.id ? { ...tool, metadata: event.metadata } : tool
          ),
        })),
        effects,
      };
    }
    case "tool_call_result": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => ({
          ...m,
          streaming: true,
          tools: m.tools.map((t) => {
            const match = event.id
              ? t.id === event.id
              : t.name === event.name &&
                (t.status === "running" || t.status === "awaiting_approval");
            if (!match) return t;
            return {
              ...t,
              status: event.isError ? "error" : "done",
              summary: event.summary,
              approvalId: undefined,
              // Keep path/diff so completed writes stay clickable in DiffViewer.
              path: event.path ?? t.path,
              diff: event.diff ?? t.diff,
              metadata: event.metadata ?? t.metadata,
            };
          }),
          question:
            m.question?.toolCallId === event.id ? undefined : m.question,
        })),
        effects,
      };
    }
    case "question_needed": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => ({
          ...m,
          question: {
            questionId: event.question_id,
            toolCallId: event.tool_call_id,
            prompt: event.prompt,
            choices: event.choices.map((value) => ({ value, label: value })),
            multiple: event.multiple,
            placeholder: event.placeholder ?? undefined,
          },
          streaming: true,
        })),
        effects,
      };
    }
    case "approval_needed": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      const toolId = event.tool_call_id;
      return {
        state: patchAssistant(ensured.state, ensured.id, (m) => {
          const exists = m.tools.some((t) => t.id === toolId);
          const nextTool: ToolPart = {
            id: toolId,
            name: event.tool_name,
            status: "awaiting_approval",
            summary: event.summary,
            approvalId: event.approval_id,
            path: event.path,
            diff: event.diff,
          };
          return {
            ...m,
            streaming: true,
            tools: exists
              ? m.tools.map((t) => (t.id === toolId ? { ...t, ...nextTool } : t))
              : [...m.tools, nextTool],
          };
        }),
        effects,
      };
    }
    case "done": {
      let next = state;
      if (event.message_id) {
        next = patchAssistant(next, event.message_id, (m) => ({
          ...m,
          streaming: false,
          question: undefined,
          providerActivity: finishProviderActivities(m.providerActivity, "done"),
        }));
      } else if (next.activeAssistantId) {
        const id = next.activeAssistantId;
        next = patchAssistant(next, id, (m) => ({
          ...m,
          streaming: false,
          question: undefined,
          providerActivity: finishProviderActivities(m.providerActivity, "done"),
        }));
      }
      return {
        state: {
          ...sweepInterruptedTools(next),
          activeAssistantId: null,
          sending: false,
          currentTurnId: null,
        },
        effects,
      };
    }
    case "cancelled": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      return {
        state: {
          ...sweepInterruptedTools(
            patchAssistant(ensured.state, ensured.id, (m) => ({
              ...m,
              streaming: false,
              question: undefined,
              providerActivity: finishProviderActivities(m.providerActivity, "error"),
              error: m.error ?? "turn cancelled",
            }))
          ),
          activeAssistantId: null,
          sending: false,
          currentTurnId: null,
        },
        effects,
      };
    }
    case "error": {
      const ensured = ensureAssistant(state, event.message_id, newId);
      effects.errorToast = event.message;
      return {
        state: {
          ...sweepInterruptedTools(
            patchAssistant(ensured.state, ensured.id, (m) => ({
              ...m,
              streaming: false,
              question: undefined,
              providerActivity: finishProviderActivities(m.providerActivity, "error"),
              error: event.message,
              // Only set when signing in again is the actual fix.
              reconnectProvider: event.reconnect_provider ?? undefined,
              providerSelection: event.provider_selection ?? undefined,
            }))
          ),
          activeAssistantId: null,
          sending: false,
          currentTurnId: null,
        },
        effects,
      };
    }
    case "warning": {
      effects.warningToast = event.message;
      return { state, effects };
    }
    case "workspace_changed": {
      // This event drives the branch-review surface in App; it must not add a
      // synthetic transcript message or alter the turn state.
      return { state, effects };
    }
  }
}

/**
 * Reduce a coalesced frame of chat events without publishing every
 * intermediate transcript to React. App uses this for text/thinking deltas;
 * the single-event reducer remains the source of truth for ordering and stale
 * event checks.
 */
export function reduceChatEvents(
  state: ChatUiState,
  events: readonly ChatEvent[],
  options?: ChatReduceOptions
): ChatReduceResult {
  let next = state;
  const effects: ChatReduceEffects = {};
  for (const event of events) {
    const reduced = reduceChatEvent(next, event, options);
    next = reduced.state;
    if (reduced.effects.errorToast) {
      effects.errorToast = reduced.effects.errorToast;
    }
    if (reduced.effects.warningToast) {
      effects.warningToast = reduced.effects.warningToast;
    }
  }
  return { state: next, effects };
}

export function initialChatUiState(
  messages: ChatMessage[] = [],
  identity?: { sessionId?: string | null; threadId?: string | null }
): ChatUiState {
  return {
    messages,
    activeAssistantId: null,
    sending: false,
    sessionId: identity?.sessionId ?? null,
    threadId: identity?.threadId ?? null,
    currentTurnId: null,
  };
}

/** Optimistic Allow: card becomes running until tool_call_result. */
export function markApprovalRunning(
  messages: ChatMessage[],
  approvalId: string
): ChatMessage[] {
  return messages.map((m) => {
    if (m.role !== "assistant") return m;
    return {
      ...m,
      tools: m.tools.map((t) =>
        t.approvalId === approvalId
          ? { ...t, status: "running" as const, approvalId: undefined }
          : t
      ),
    };
  });
}

/** Restore approval card after resolve_approval failed. */
export function restoreApprovalCard(
  messages: ChatMessage[],
  snapshot: ToolPart
): ChatMessage[] {
  return messages.map((m) => {
    if (m.role !== "assistant") return m;
    const idx = m.tools.findIndex((t) => t.id === snapshot.id);
    if (idx < 0) return m;
    const tools = [...m.tools];
    tools[idx] = {
      ...snapshot,
      status: "awaiting_approval",
      approvalId: snapshot.approvalId,
    };
    return { ...m, tools };
  });
}

/**
 * Retire a card whose waiter the backend no longer has.
 *
 * The counterpart to {@link restoreApprovalCard}: restoring is right when the
 * resolve failed for a transient reason and the user should try again, but a
 * dropped waiter is permanent. Putting that card back leaves a button that can
 * only ever fail, sitting ahead of the live approval in the queue.
 */
export function retireApprovalCard(
  messages: ChatMessage[],
  approvalId: string
): ChatMessage[] {
  return messages.map((m) => {
    if (m.role !== "assistant") return m;
    if (!m.tools.some((t) => t.approvalId === approvalId)) return m;
    return {
      ...m,
      tools: m.tools.map((t) =>
        t.approvalId === approvalId
          ? {
              ...t,
              status: "error" as const,
              approvalId: undefined,
              summary: t.summary
                ? `${t.summary} (approval expired)`
                : "approval expired",
            }
          : t
      ),
    };
  });
}

/** Find the tool card for an approval id (for failure restore). */
export function findApprovalTool(
  messages: ChatMessage[],
  approvalId: string
): ToolPart | null {
  for (const m of messages) {
    if (m.role !== "assistant") continue;
    const tool = m.tools.find((t) => t.approvalId === approvalId);
    if (tool) return { ...tool };
  }
  return null;
}
