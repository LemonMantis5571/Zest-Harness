# Temporary side conversations

Use `/btw [question]` in the desktop's main composer or interactive terminal to
ask about the current conversation without appending the exchange to the main
chat. The command must be the first complete token; paths such as `/btw/file`
and inline mentions remain ordinary text.

The desktop streams the answer into a separate panel. You can ask follow-up
questions, stop its answer, or close it with Esc. In the terminal, `/back`
discards the side conversation. Reopening `/btw` starts a new side conversation.
Attachments in the main composer remain there; they are not sent as part of
the side question.

## Context and providers

The side conversation freezes a persistence-safe snapshot of the main context
at the moment it opens. When the main turn is active, that snapshot includes the
submitted user prompt and provider/tool rounds completed so far; it does not
include the response currently streaming or work completed after the panel was
opened. Sensitive tool results use the same redaction rules as the persisted
history. It retains the system, tool definition, model, effort, and message
prefix when making native API requests, and appends side-specific instructions
after that prefix. It never runs Zest's local tool loop or changes the main
chat's context meter, checkpoints, pending inputs, or provider cursor.

| Connection | Side conversation |
| --- | --- |
| Native API / compatible endpoint | Sends the frozen transcript with tool execution disabled; any prompt caching follows the endpoint's rules. |
| Codex CLI | Requests an ephemeral `thread/fork` from the idle parent and resumes the resulting child. If the fork cannot be restored, starts an ephemeral thread with the frozen transcript. Uses a read-only sandbox without approval grants. |
| Claude Code | Uses `--resume` with `--fork-session`, then resumes the child for follow-ups. Built-in tools, MCP servers, and slash-command execution are disabled for the side request. |
| Cursor ACP | Uses a separate process and session in Ask mode with the frozen transcript, preserving the parent's live connection. |

Claude's fork and tool flags follow its [CLI reference](https://code.claude.com/docs/en/cli-reference).

If the main task is already running when the panel opens, the side question uses
the frozen snapshot published by the main worker in a separate provider
session. Zest does not fork or share the moving provider cursor while that
session is receiving new work. The main task continues independently.

Cache eligibility, expiration, and billing remain provider decisions. A shared
prefix can reduce input cost, but changing provider controls may affect cache
eligibility. `/btw` does not promise that only the latest message is billed.
Reported side-question usage still goes into the normal usage ledger.

## Lifetime and failure handling

Zest keeps the side exchange in memory, outside its saved threads and recovery
log. Closing the panel, switching away from its session, or exiting Zest drops
that exchange. The connected provider or coding CLI may keep its own session
records under its normal retention behavior.

Errors and cancellation do not commit the failed side turn. Its question stays
editable in the desktop panel, and retrying rebuilds from the last successful
side transcript with a fresh provider cursor. Closing a running panel cancels
only that request. Its late output cannot reopen the panel or enter the main
chat.
