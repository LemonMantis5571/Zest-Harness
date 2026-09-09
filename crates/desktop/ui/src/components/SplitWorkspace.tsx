import { useEffect, useRef, useState } from "react";
import { ArrowUpIcon, Columns2Icon, GitForkIcon, Maximize2Icon, SquareIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Markdown } from "@/components/Markdown";
import { ToolCallRow } from "@/components/ToolCallRow";
import { PlanningQuestionnaire } from "@/components/PlanningQuestionnaire";
import { getBackend } from "@/lib/backend";
import { loadDraft } from "@/lib/drafts";
import type { ChatUiState } from "@/lib/chatReducer";
import type { ProjectChats, SessionInfo } from "@/lib/types";

export type SplitChatTarget = { root: string | null; threadId?: string; newThread?: boolean; providerId?: string };
type SplitPane = { session: SessionInfo; target: SplitChatTarget };
type Props = {
  initial: SessionInfo;
  initialDraft: string;
  states: ReadonlyMap<string, ChatUiState>;
  onOpen: (target: SplitChatTarget, fork?: boolean) => Promise<SessionInfo>;
  onSend: (session: SessionInfo, target: SplitChatTarget, text: string) => Promise<SessionInfo | void>;
  onClose: (session: SessionInfo, draft: string, target: SplitChatTarget) => Promise<void>;
};

function targetFor(session: SessionInfo): SplitChatTarget {
  return { root: session.isFreeChat ? null : session.root, threadId: session.threadId };
}

function initialTargetFor(session: SessionInfo): SplitChatTarget {
  const hasTranscript = session.messages.length > 0 || session.hasOlderMessages || session.hasNewerMessages;
  return hasTranscript
    ? targetFor(session)
    : { root: session.isFreeChat ? null : session.root, newThread: true, providerId: session.provider };
}

function readSplitDraft(id: string) {
  try { return localStorage.getItem(`zest.splitDraft.${id}`) ?? loadDraft(id); }
  catch { return loadDraft(id); }
}

export function SplitWorkspace({ initial, initialDraft, states, onOpen, onSend, onClose }: Props) {
  const [panes, setPanes] = useState<[SplitPane | null, SplitPane | null]>([
    { session: initial, target: initialTargetFor(initial) },
    null,
  ]);
  const [projects, setProjects] = useState<ProjectChats[]>([]);
  const [drafts, setDrafts] = useState<Record<string, string>>({ [initial.threadId]: initialDraft });
  const [choosing, setChoosing] = useState<number | null>(1);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [ratio, setRatio] = useState(50);
  const container = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    getBackend().listChatProjects().then((next) => { if (!cancelled) setProjects(next); })
      .catch(() => { if (!cancelled) setError("Could not load chats. Close split view and try again."); });
    return () => { cancelled = true; };
  }, []);

  function updateDraft(id: string, value: string) {
    setDrafts((current) => ({ ...current, [id]: value }));
    try { localStorage.setItem(`zest.splitDraft.${id}`, value); } catch { /* Draft stays in memory. */ }
  }

  async function run(action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try { await action(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { busyRef.current = false; setBusy(false); }
  }

  async function choose(index: number, target: SplitChatTarget, fork = false) {
    await run(async () => {
      const info = await onOpen(target, fork);
      const paneTarget = target.newThread && !fork
        ? { root: target.root, newThread: true, providerId: target.providerId }
        : targetFor(info);
      setPanes((current) => index === 0
        ? [{ session: info, target: paneTarget }, current[1]]
        : [current[0], { session: info, target: paneTarget }]);
      setDrafts((current) => ({ ...current, [info.threadId]: current[info.threadId] ?? readSplitDraft(info.threadId) }));
      setChoosing(null);
      setQuery("");
    });
  }

  return (
    <section aria-label="Split workspace" className="flex min-h-0 flex-1 flex-col bg-[var(--canvas-solid)]">
      <header className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2">
        <Columns2Icon className="size-4 text-muted-foreground" aria-hidden="true" />
        <h1 className="text-sm font-medium">Split view</h1>
        <span className="hidden text-xs text-muted-foreground sm:block">Two chats, separate drafts</span>
        <Button className="ml-auto" variant="ghost" size="sm" disabled={busy} onClick={() => {
          const pane = panes[0];
          const session = pane?.session ?? initial;
          void run(() => onClose(session, drafts[session.threadId] ?? "", pane?.target ?? targetFor(session)));
        }}>Close split</Button>
      </header>
      {error ? <p role="alert" className="border-b border-border px-4 py-2 text-sm text-destructive">{error}</p> : null}
      <div ref={container} className="zest-split-panes flex min-h-0 flex-1" style={{ "--split-left": `${ratio}%` } as React.CSSProperties}>
        {panes.map((pane, index) => {
          const session = pane?.session ?? null;
          const target = pane?.target;
          return (
          <div key={index} className="contents">
            {index === 1 ? <div role="separator" aria-label="Resize split panes" aria-orientation="vertical" aria-valuemin={30} aria-valuemax={70} aria-valuenow={ratio} tabIndex={0}
              className="zest-split-divider w-2 shrink-0 cursor-col-resize touch-none bg-border/40 hover:bg-primary/40 focus-visible:bg-primary/40 focus-visible:outline-none"
              onDoubleClick={() => setRatio(50)}
              onKeyDown={(event) => {
                if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
                event.preventDefault();
                setRatio((value) => event.key === "Home" ? 30 : event.key === "End" ? 70 : Math.max(30, Math.min(70, value + (event.key === "ArrowLeft" ? -2 : 2))));
              }}
              onPointerDown={(event) => { event.currentTarget.setPointerCapture(event.pointerId); }}
              onPointerMove={(event) => {
                if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
                const rect = container.current?.getBoundingClientRect();
                if (rect) setRatio(Math.max(30, Math.min(70, (event.clientX - rect.left) / rect.width * 100)));
              }}
              onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }} /> : null}
            <section aria-label={index === 0 ? "Left chat" : "Right chat"} className="zest-split-pane flex min-h-0 min-w-0 flex-col" style={{ flex: index === 0 ? "0 0 calc(var(--split-left) - 4px)" : "1" }}>
              <header className="flex min-w-0 shrink-0 items-center gap-1 border-b border-border/60 p-2">
                <button type="button" disabled={busy} onClick={() => { setChoosing(index); setQuery(""); }} className="min-w-0 flex-1 rounded-md px-2 py-1 text-left hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  <span className="block truncate text-sm font-medium">{session ? projects.flatMap((project) => project.threads).find((thread) => thread.id === session.threadId)?.title || (index === 0 ? "First chat" : "Second chat") : "Choose a chat"}</span>
                  <span className="block truncate text-xs text-muted-foreground">{session ? `${session.isFreeChat ? "No workspace" : session.root.split(/[\\/]/).at(-1)} · ${session.label} · ${session.model}` : "Open a conversation alongside the first"}</span>
                </button>
                {session ? <>
                  <Button variant="ghost" size="icon-sm" title="Fork into other pane" aria-label="Fork into other pane" disabled={busy || states.get(session.threadId)?.sending} onClick={() => void choose(1 - index, target ?? targetFor(session), true)}><GitForkIcon aria-hidden="true" /></Button>
                  <Button variant="ghost" size="icon-sm" title="Continue in single view" aria-label="Continue in single view" disabled={busy} onClick={() => void run(() => onClose(session, drafts[session.threadId] ?? "", target ?? targetFor(session)))}><Maximize2Icon aria-hidden="true" /></Button>
                </> : null}
              </header>
              {choosing === index || !session ? (
                <div className="min-h-0 flex-1 overflow-y-auto p-4">
                  <div className="mb-3 flex gap-2">
                    <input aria-label="Find a chat" placeholder="Find a chat or project" value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-0 flex-1 rounded-md border border-border bg-background px-3 py-2 text-sm" />
                    {session ? <Button variant="ghost" size="sm" onClick={() => setChoosing(null)}>Cancel</Button> : null}
                  </div>
                  <Button variant="outline" size="sm" disabled={busy} onClick={() => void choose(index, { root: initial.isFreeChat ? null : initial.root, newThread: true, providerId: initial.provider })}>New chat in this project</Button>
                  <div className="mt-4 flex flex-col gap-1">
                    {projects.flatMap((project) => project.threads.filter((thread) => thread.id !== panes[1 - index]?.session.threadId && `${project.name} ${thread.title ?? ""}`.toLowerCase().includes(query.toLowerCase())).map((thread) => (
                      <button key={`${project.path}:${thread.id}`} type="button" disabled={busy} className="rounded-lg px-3 py-2 text-left hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => void choose(index, { root: project.path, threadId: thread.id })}>
                        <span className="block truncate text-sm">{thread.title || "Untitled chat"}</span><span className="block truncate text-xs text-muted-foreground">{project.name}</span>
                      </button>
                    )))}
                  </div>
                </div>
              ) : (
                <SplitConversation key={session.threadId} session={session} state={states.get(session.threadId)} draft={drafts[session.threadId] ?? ""} busy={busy}
                  onDraft={(value) => updateDraft(session.threadId, value)}
                  onSend={() => run(async () => {
                    const text = drafts[session.threadId]?.trim();
                    if (!text) return;
                    const next = await onSend(session, target ?? targetFor(session), text);
                    const nextSession = next ?? session;
                    if (next) {
                      setPanes((current) => index === 0
                        ? [{ session: nextSession, target: targetFor(nextSession) }, current[1]]
                        : [current[0], { session: nextSession, target: targetFor(nextSession) }]);
                    }
                    if (nextSession.threadId !== session.threadId) updateDraft(session.threadId, "");
                    updateDraft(nextSession.threadId, "");
                  })}
                  onStop={() => void run(() => getBackend().cancelTurn(session.threadId))}
                  onError={(message) => setError(message)} />
              )}
            </section>
          </div>
          );
        })}
      </div>
    </section>
  );
}

function SplitConversation({ session, state, draft, busy, onDraft, onSend, onStop, onError }: {
  session: SessionInfo; state?: ChatUiState; draft: string; busy: boolean;
  onDraft: (value: string) => void; onSend: () => Promise<void>; onStop: () => void; onError: (value: string) => void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const messages = state?.messages ?? session.messages;
  const sending = state?.sending ?? false;
  useEffect(() => { if (follow.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight; }, [messages]);
  return <>
    <div ref={viewport} className="min-h-0 flex-1 overflow-y-auto px-5 py-4" onScroll={() => { const node = viewport.current; if (node) follow.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80; }}>
      {session.hasOlderMessages ? <p className="mb-4 text-xs text-muted-foreground">Showing recent messages. Continue in single view to load earlier history.</p> : null}
      {!messages.length ? <p className="py-12 text-center text-sm text-muted-foreground">Start a conversation.</p> : null}
      {messages.map((message) => <article key={message.id} className="mb-6 min-w-0 break-words">
        <p className="mb-2 text-xs font-medium text-muted-foreground">{message.role === "user" ? "You" : session.label}</p>
        {message.role === "user" ? <div className="whitespace-pre-wrap rounded-xl bg-secondary p-3 text-sm">{message.text}{message.attachments?.map((item) => <span key={item.name} className="block text-xs text-muted-foreground">{item.name}</span>)}</div> : <>
          {message.thinking ? <details className="mb-2 text-xs text-muted-foreground"><summary>Thinking</summary><p className="whitespace-pre-wrap py-2">{message.thinking}</p></details> : null}
          <Markdown streaming={message.streaming}>{message.text}</Markdown>
          {message.tools.map((tool) => <ToolCallRow key={tool.id} tool={tool} asCard={tool.status === "awaiting_approval"} onResolveApproval={async (id, choice) => { try { await getBackend().resolveApproval(id, choice, session.threadId); } catch (error) { onError(String(error)); throw error; } }} />)}
          {message.question?.questionId ? <PlanningQuestionnaire question={message.question} onSubmit={async (answer) => { try { await getBackend().resolveQuestion(message.question!.questionId!, answer, session.threadId); } catch (error) { onError(String(error)); throw error; } }} /> : null}
          {message.error ? <p role="alert" className="text-sm text-destructive">{message.error}</p> : null}
        </>}
      </article>)}
    </div>
    <form className="m-3 shrink-0 rounded-xl border border-border bg-card p-3 focus-within:ring-1 focus-within:ring-ring/40" onSubmit={(event) => { event.preventDefault(); if (!busy && !sending) onSend(); }}>
      <textarea aria-label="Message" placeholder="Ask anything…" value={draft} onChange={(event) => onDraft(event.target.value)} rows={3} className="w-full resize-none bg-transparent text-sm outline-none" onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (!busy && !sending) onSend(); } }} />
      <div className="flex items-center justify-between gap-2"><span role="status" className="truncate text-xs text-muted-foreground">{sending ? "Working…" : session.model}</span>
        {sending ? <Button type="button" size="icon-sm" aria-label="Stop response" disabled={busy} onClick={onStop}><SquareIcon aria-hidden="true" /></Button> : <Button type="submit" size="icon-sm" aria-label="Send message" disabled={busy || !draft.trim()}><ArrowUpIcon aria-hidden="true" /></Button>}
      </div>
    </form>
  </>;
}
