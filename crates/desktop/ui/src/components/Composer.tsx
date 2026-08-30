import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import {
  ArrowUpIcon,
  CheckIcon,
  FileIcon,
  FileTextIcon,
  FolderOpenIcon,
  GitBranchIcon,
  GitPullRequestIcon,
  ImageIcon,
  PencilIcon,
  PlusIcon,
  SquareIcon,
  Trash2Icon,
  XIcon,
  ZapIcon,
} from "lucide-react";

import { ApprovalModePicker } from "@/components/ApprovalModePicker";
import { ContextUsageButton } from "@/components/ContextUsageButton";
import { ModelEffortPicker } from "@/components/ModelEffortPicker";
import {
  Attachment,
  AttachmentAction,
  AttachmentActions,
  AttachmentContent,
  AttachmentDescription,
  AttachmentGroup,
  AttachmentMedia,
  AttachmentTitle,
} from "@/components/ui/attachment";
import { Button } from "@/components/ui/button";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { IconSwap } from "@/components/ui/icon-swap";
import {
  chipLabel,
  effortsForModel,
  modelLabel,
  type EffortId,
  type ModelCapability,
} from "@/lib/models";
import { getBackend } from "@/lib/backend";
import type {
  ApprovalMode,
  CommandView,
  GitContext,
  PreparedAttachment,
  ProviderRow,
} from "@/lib/types";
import { hasResumableThreadTurn, type QueuedTurn } from "@/lib/threadQueue";
import {
  filterSlashCommands,
  isModelCommandName,
  splitSlashMatch,
} from "@/lib/slashCommands";
import { cn } from "@/lib/utils";

type Props = {
  value: string;
  model: string;
  effort: EffortId;
  models?: ModelCapability[];
  defaultModel?: string;
  folderLabel: string;
  branch: string | null;
  gitContext: GitContext | null;
  approvalMode: ApprovalMode;
  contextRefreshKey: string | number;
  sending: boolean;
  queuedMessages: ReadonlyArray<QueuedTurn>;
  onUpdateQueuedMessage: (turnId: string, text: string) => void;
  onRemoveQueuedMessage: (turnId: string) => void;
  onResumeQueuedMessages?: () => void;
  resumingQueuedMessages?: boolean;
  showModelPicker: boolean;
  currentProviderId?: string;
  currentProviderLabel?: string;
  providers?: ProviderRow[];
  modelPickerOpen?: boolean;
  onModelPickerOpenChange?: (open: boolean) => void;
  onSwitchProvider?: (providerId: string, model: string) => void;
  optionsDisabled?: boolean;
  attachments: PreparedAttachment[];
  onChange: (value: string) => void;
  onSubmit: () => void;
  onStop?: () => void;
  onApprovalModeChange: (mode: ApprovalMode) => void;
  onModelChange: (model: string) => void;
  onEffortChange: (effort: EffortId) => void;
  onResetOptions?: () => void;
  onAttachFiles: () => void;
  onOpenFolder: () => void;
  onRemoveAttachment: (id: string) => void;
  onPasteImages: (files: File[]) => void;
  compacting?: boolean;
  /** Sticky chrome above the input (e.g. pending approvals). */
  aboveComposer?: ReactNode;
};

function attachmentPreviewUrl(att: PreparedAttachment): string | null {
  if (att.kind !== "image" || !att.dataBase64 || !att.mediaType) return null;
  return `data:${att.mediaType};base64,${att.dataBase64}`;
}

export function Composer({
  value,
  model,
  effort,
  models,
  defaultModel,
  folderLabel,
  branch,
  gitContext,
  approvalMode,
  contextRefreshKey,
  sending,
  queuedMessages,
  onUpdateQueuedMessage,
  onRemoveQueuedMessage,
  onResumeQueuedMessages,
  resumingQueuedMessages = false,
  showModelPicker,
  currentProviderId,
  currentProviderLabel,
  providers,
  modelPickerOpen,
  onModelPickerOpenChange,
  onSwitchProvider,
  optionsDisabled = false,
  attachments,
  onChange,
  onSubmit,
  onStop,
  onApprovalModeChange,
  onModelChange,
  onEffortChange,
  onResetOptions,
  onAttachFiles,
  onOpenFolder,
  onRemoveAttachment,
  onPasteImages,
  compacting = false,
  aboveComposer,
}: Props) {
  const supportsEffort = effortsForModel(models, model).length > 0;
  const canResumeQueued = hasResumableThreadTurn(queuedMessages);
  const ref = useRef<HTMLTextAreaElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuId = useId();

  const [commands, setCommands] = useState<CommandView[]>([]);
  const [commandIndex, setCommandIndex] = useState(0);
  const [commandsDismissed, setCommandsDismissed] = useState(false);
  const [editingQueuedId, setEditingQueuedId] = useState<string | null>(null);
  const [editingQueuedText, setEditingQueuedText] = useState("");

  useEffect(() => {
    let cancelled = false;
    void getBackend()
      .listCommands()
      .then((next) => {
        if (!cancelled) setCommands(next);
      })
      .catch((error) => {
        // No commands is a normal state, not an error worth a toast.
        ignoreExpectedFailure(error, "load composer commands");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Only a token being typed at the very start opens the palette — the same
  // rule the Rust parser uses, so what you see matches what will run.
  const typedCommand = /^\/([a-z0-9-_]*)$/i.exec(value.trimStart())?.[1];
  const slashOpen = typedCommand !== undefined && !commandsDismissed;
  const commandMatches = slashOpen
    ? filterSlashCommands(commands, typedCommand)
    : [];

  useEffect(() => {
    setCommandIndex(0);
  }, [typedCommand]);

  useEffect(() => {
    if (typedCommand === undefined) setCommandsDismissed(false);
  }, [typedCommand]);

  // Reload when the palette opens so an MCP added in Customize is in the list.
  useEffect(() => {
    if (!slashOpen) return;
    let cancelled = false;
    void getBackend()
      .listCommands()
      .then((next) => {
        if (!cancelled) setCommands(next);
      })
      .catch((error) => {
        ignoreExpectedFailure(error, "reload composer commands");
      });
    return () => {
      cancelled = true;
    };
  }, [slashOpen]);

  function applyCommand(command: CommandView) {
    if (command.kind === "builtin" && isModelCommandName(command.name)) {
      onChange("");
      setCommandsDismissed(true);
      onModelPickerOpenChange?.(true);
      ref.current?.focus();
      return;
    }
    onChange(`/${command.name} `);
    setCommandsDismissed(true);
    ref.current?.focus();
  }

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
  }, [value]);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      const root = menuRef.current;
      if (!root) return;
      if (event.target instanceof Node && !root.contains(event.target)) {
        setMenuOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [menuOpen]);

  const hasOkAttachment = attachments.some(
    (a) =>
      a.status === "done" &&
      (Boolean(a.content?.trim()) ||
        (a.kind === "image" && Boolean(a.dataBase64)))
  );
  const canSend =
    !compacting && (value.trim().length > 0 || hasOkAttachment);

  useEffect(() => {
    if (
      editingQueuedId &&
      !queuedMessages.some((turn) => turn.id === editingQueuedId)
    ) {
      setEditingQueuedId(null);
      setEditingQueuedText("");
    }
  }, [editingQueuedId, queuedMessages]);

  function beginQueuedEdit(turn: QueuedTurn) {
    setEditingQueuedId(turn.id);
    setEditingQueuedText(turn.text);
  }

  function cancelQueuedEdit() {
    setEditingQueuedId(null);
    setEditingQueuedText("");
  }

  function saveQueuedEdit() {
    if (!editingQueuedId) return;
    const current = queuedMessages.find((turn) => turn.id === editingQueuedId);
    if (!current) {
      cancelQueuedEdit();
      return;
    }

    const text = editingQueuedText.trim();
    if (!text && current.attachments.length === 0) return;
    onUpdateQueuedMessage(editingQueuedId, text);
    cancelQueuedEdit();
  }

  return (
    <div className="pointer-events-none absolute inset-x-0 bottom-0 z-10 px-4 pb-3 pt-20">
      <div className="pointer-events-auto mx-auto w-full max-w-[var(--chat-max)]">
        {aboveComposer}
        {queuedMessages.length > 0 ? (
          <div
            className="pointer-events-auto mb-2 rounded-xl border border-border/80 bg-[color-mix(in_srgb,var(--card)_92%,transparent)] p-2 shadow-lg backdrop-blur-xl"
            aria-live="polite"
          >
            <div className="flex items-center justify-between gap-2 px-1 pb-1.5 text-[11px] text-muted-foreground">
              <span className="font-medium uppercase tracking-wide">
                Queued messages
              </span>
              <div className="flex items-center gap-2">
                <span className="tabular-nums">{queuedMessages.length}</span>
                {onResumeQueuedMessages && canResumeQueued ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="xs"
                    disabled={sending || compacting || resumingQueuedMessages}
                    onClick={onResumeQueuedMessages}
                  >
                    {resumingQueuedMessages ? "Resuming…" : "Resume queued"}
                  </Button>
                ) : null}
              </div>
            </div>
            <div className="space-y-1">
              {queuedMessages.map((turn, index) => {
                const editing = editingQueuedId === turn.id;
                return (
                  <div
                    key={turn.id}
                    className="rounded-lg border border-border/60 bg-background/30 p-1.5"
                  >
                    {editing ? (
                      <div className="flex items-end gap-1.5">
                        <textarea
                          rows={2}
                          value={editingQueuedText}
                          aria-label={`Edit queued message ${index + 1}`}
                          className="min-h-10 flex-1 resize-none rounded-md bg-transparent px-2 py-1.5 text-xs text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
                          onChange={(event) =>
                            setEditingQueuedText(event.target.value)
                          }
                          onKeyDown={(event) => {
                            if (event.key === "Escape") {
                              event.preventDefault();
                              cancelQueuedEdit();
                            } else if (
                              event.key === "Enter" &&
                              !event.shiftKey &&
                              !event.nativeEvent.isComposing
                            ) {
                              event.preventDefault();
                              saveQueuedEdit();
                            }
                          }}
                        />
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          aria-label={`Save queued message ${index + 1}`}
                          title="Save queued message"
                          onClick={saveQueuedEdit}
                        >
                          <CheckIcon className="size-3.5" />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          aria-label={`Cancel editing queued message ${index + 1}`}
                          title="Cancel editing"
                          onClick={cancelQueuedEdit}
                        >
                          <XIcon className="size-3.5" />
                        </Button>
                      </div>
                    ) : (
                      <div className="flex items-center gap-1.5">
                        <div
                          className="min-w-0 flex-1 truncate text-xs text-foreground"
                          title={turn.text || "Attachment-only message"}
                        >
                          <span className="mr-1 text-muted-foreground">
                            {index + 1}.
                          </span>
                          {turn.target && turn.target !== "followup" ? (
                            <span className="mr-1 rounded bg-muted px-1 text-[9px] uppercase tracking-wide text-muted-foreground">
                              {turn.target}
                            </span>
                          ) : null}
                          {turn.text || "Attachment-only message"}
                          {turn.attachments.length > 0 ? (
                            <span className="ml-1 text-[10px] text-muted-foreground">
                              · {turn.attachments.length} attachment
                              {turn.attachments.length === 1 ? "" : "s"}
                            </span>
                          ) : null}
                        </div>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          aria-label={`Edit queued message ${index + 1}`}
                          title="Edit queued message"
                          onClick={() => beginQueuedEdit(turn)}
                        >
                          <PencilIcon className="size-3.5" />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          aria-label={`Remove queued message ${index + 1}`}
                          title="Remove queued message"
                          onClick={() => onRemoveQueuedMessage(turn.id)}
                        >
                          <Trash2Icon className="size-3.5" />
                        </Button>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        ) : null}
        <div className="overflow-visible rounded-2xl border border-border bg-[color-mix(in_srgb,var(--card)_92%,transparent)] shadow-[0_16px_48px_rgba(0,0,0,0.55)] backdrop-blur-xl">
          {attachments.length > 0 ? (
            <AttachmentGroup className="px-3 pt-3">
              {attachments.map((att) => {
                const preview = attachmentPreviewUrl(att);
                return (
                  <Attachment
                    key={att.id}
                    size="sm"
                    state={att.status === "error" ? "error" : "done"}
                  >
                    <AttachmentMedia variant={preview ? "image" : "icon"}>
                      {preview ? (
                        <img src={preview} alt="" />
                      ) : att.kind === "pdf" ? (
                        <FileTextIcon />
                      ) : att.kind === "image" ? (
                        <ImageIcon />
                      ) : (
                        <FileIcon />
                      )}
                    </AttachmentMedia>
                    <AttachmentContent>
                      <AttachmentTitle>{att.name}</AttachmentTitle>
                      <AttachmentDescription>{att.detail}</AttachmentDescription>
                    </AttachmentContent>
                    <AttachmentActions>
                      <AttachmentAction
                        type="button"
                        title="Remove"
                        onClick={() => onRemoveAttachment(att.id)}
                      >
                        <XIcon />
                      </AttachmentAction>
                    </AttachmentActions>
                  </Attachment>
                );
              })}
            </AttachmentGroup>
          ) : null}
          {commandMatches.length > 0 ? (
            <div
              role="listbox"
              aria-label="Commands"
              className="mx-2 mt-2 overflow-hidden rounded-xl border border-white/[0.08] bg-popover/95 shadow-xl"
            >
              {commandMatches.map((cmd, index) => {
                const selected = index === commandIndex;
                const parts = splitSlashMatch(cmd.name, typedCommand ?? "");
                return (
                  <button
                    key={`${cmd.kind}:${cmd.name}`}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    onMouseEnter={() => setCommandIndex(index)}
                    onClick={() => applyCommand(cmd)}
                    className={cn(
                      "flex w-full items-center gap-2.5 px-2.5 py-1.5 text-left transition-colors",
                      selected ? "bg-foreground/10" : "hover:bg-foreground/5"
                    )}
                  >
                    <ZapIcon
                      className="size-3.5 shrink-0 text-emerald-400"
                      aria-hidden
                    />
                    <span className="shrink-0 text-[13px] font-medium text-foreground">
                      {parts.prefix}
                      {parts.match ? (
                        <span className="text-sky-300">{parts.match}</span>
                      ) : null}
                      {parts.suffix}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-[12px] text-muted-foreground">
                      {cmd.description}
                    </span>
                    {selected ? (
                      <span className="shrink-0 text-[11px] text-muted-foreground/80">
                        Enter
                      </span>
                    ) : null}
                  </button>
                );
              })}
            </div>
          ) : null}
          <textarea
            ref={ref}
            id="zest-composer-input"
            rows={1}
            value={value}
            placeholder="Ask about this project — / for commands, paste or attach files"
            autoComplete="off"
            className="block max-h-[180px] w-full resize-none bg-transparent px-4 pt-3.5 pb-2 text-sm text-foreground caret-foreground outline-none placeholder:text-muted-foreground"
            onChange={(e) => onChange(e.target.value)}
            onPaste={(e) => {
              const items = Array.from(e.clipboardData?.items ?? []);
              const imageFiles = items
                .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
                .map((item) => item.getAsFile())
                .filter((f): f is File => Boolean(f));
              if (imageFiles.length === 0) return;
              e.preventDefault();
              onPasteImages(imageFiles);
            }}
            onKeyDown={(e) => {
              // The palette owns these keys while it is open, so Enter picks a
              // command instead of sending a half-typed `/pl`.
              if (commandMatches.length > 0) {
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  const step = e.key === "ArrowDown" ? 1 : -1;
                  setCommandIndex(
                    (i) =>
                      (i + step + commandMatches.length) % commandMatches.length
                  );
                  return;
                }
                if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
                  e.preventDefault();
                  const selected = commandMatches[commandIndex];
                  if (selected) applyCommand(selected);
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  setCommandsDismissed(true);
                  return;
                }
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                if (canSend) onSubmit();
              }
            }}
          />
          <div className="flex items-center justify-between gap-2 px-2.5 pb-2.5">
            <div className="relative z-20 flex min-w-0 items-center gap-1 overflow-visible">
              <div ref={menuRef} className="relative">
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  title="Add context"
                  aria-haspopup="menu"
                  aria-expanded={menuOpen}
                  aria-controls={menuOpen ? menuId : undefined}
                  className={cn(
                    "text-muted-foreground",
                    menuOpen && "bg-secondary text-foreground"
                  )}
                  onClick={() => setMenuOpen((v) => !v)}
                >
                  <PlusIcon />
                </Button>
                {menuOpen ? (
                  <div
                    id={menuId}
                    role="menu"
                    className="absolute bottom-[calc(100%+8px)] left-0 z-50 w-[200px] rounded-lg border border-border bg-popover p-1 text-popover-foreground shadow-lg"
                  >
                    <button
                      type="button"
                      role="menuitem"
                      className="flex w-full cursor-pointer items-center gap-2 rounded-md px-2.5 py-2 text-left text-sm outline-none hover:bg-accent"
                      onClick={() => {
                        setMenuOpen(false);
                        onAttachFiles();
                      }}
                    >
                      <FileIcon className="size-3.5 text-muted-foreground" />
                      Upload files
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      className="flex w-full cursor-pointer items-center gap-2 rounded-md px-2.5 py-2 text-left text-sm outline-none hover:bg-accent"
                      onClick={() => {
                        setMenuOpen(false);
                        onOpenFolder();
                      }}
                    >
                      <FolderOpenIcon className="size-3.5 text-muted-foreground" />
                      Open folder
                    </button>
                  </div>
                ) : null}
              </div>
              {showModelPicker ? (
                <ModelEffortPicker
                  model={model}
                  effort={effort}
                  models={models}
                  defaultModel={defaultModel}
                  currentProviderId={currentProviderId}
                  currentProviderLabel={currentProviderLabel}
                  providers={providers}
                  open={modelPickerOpen}
                  onOpenChange={onModelPickerOpenChange}
                  disabled={sending || compacting || optionsDisabled}
                  onModelChange={onModelChange}
                  onEffortChange={onEffortChange}
                  onSwitchProvider={onSwitchProvider}
                  onReset={onResetOptions}
                />
              ) : (
                <span className="truncate px-2 py-1 text-xs text-muted-foreground">
                  {supportsEffort ? chipLabel(model, effort) : modelLabel(model)}
                </span>
              )}
            </div>
            {/*
              One button for both states, not a ternary over two. Swapping the
              element unmounted whichever was focused, so starting a turn from
              the keyboard dropped focus to the body — and a remount cannot be
              cross-faded anyway.
            */}
            <Button
              type="button"
              size="icon-sm"
              disabled={!sending && !canSend}
              aria-label={sending ? "Stop" : "Send"}
              title={sending ? "Stop (Esc or Ctrl+.)" : undefined}
              className="rounded-full"
              onClick={() => {
                if (sending) {
                  onStop?.();
                  return;
                }
                if (canSend) onSubmit();
              }}
            >
              <IconSwap
                className="size-3.5"
                active={sending}
                initial={<ArrowUpIcon className="size-3.5" />}
                swapped={<SquareIcon className="size-3.5 fill-current" />}
              />
            </Button>
          </div>
        </div>
        <div className="mt-2 flex items-center justify-between gap-2 px-1 text-[11px] text-muted-foreground">
          <div className="flex min-w-0 items-center gap-2">
            <button
              type="button"
              title={folderLabel}
              onClick={onOpenFolder}
              className="inline-flex max-w-[28ch] cursor-pointer items-center gap-1 truncate rounded-md px-1 py-0.5 hover:bg-secondary hover:text-foreground"
            >
              <FolderOpenIcon className="size-3 shrink-0 opacity-70" />
              <span className="truncate">{folderLabel}</span>
            </button>
            {branch ? (
              <span
                className="inline-flex items-center gap-1 truncate"
                title={
                  gitContext?.branchChanged
                    ? `${branch} — this chat started on ${gitContext.baseBranch ?? "another branch"}.`
                    : branch
                }
              >
                <GitBranchIcon className="size-3 shrink-0 opacity-70" />
                <span className="truncate">{branch}</span>
              </span>
            ) : null}
            {gitContext?.pullRequest ? (
              <a
                href={gitContext.pullRequest.url}
                target="_blank"
                rel="noreferrer"
                title={`${gitContext.pullRequest.title} · +${gitContext.additions} −${gitContext.deletions} · ${gitContext.changedFiles} files`}
                className="inline-flex shrink-0 items-center gap-1 rounded-md px-1 py-0.5 text-muted-foreground hover:bg-secondary hover:text-foreground"
              >
                <GitPullRequestIcon className="size-3 opacity-80" />
                <span>#{gitContext.pullRequest.number}</span>
                <span className="text-primary">
                  +{gitContext.additions}
                </span>
                <span className="text-destructive">
                  −{gitContext.deletions}
                </span>
              </a>
            ) : null}
            {showModelPicker ? (
              <span className="hidden truncate sm:inline">· {modelLabel(model)}</span>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <ApprovalModePicker
              mode={approvalMode}
              disabled={optionsDisabled || compacting}
              onModeChange={onApprovalModeChange}
            />
            <ContextUsageButton
              refreshKey={contextRefreshKey}
              className="shrink-0"
            />
          </div>
        </div>
      </div>
    </div>
  );
}
