import { useEffect, useId, useRef } from "react";
import { CheckIcon, CopyIcon, Settings2Icon, XIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { ConversationRecovery } from "@/lib/invokeErrors";
import { useDialogFocusTrap } from "@/lib/useDialogFocusTrap";
import { cn } from "@/lib/utils";

type Props = {
  recovery: ConversationRecovery | null;
  busy?: boolean;
  onClose: () => void;
  onConfigure: () => void;
  onChooseProvider: (providerId: string) => void;
};

export function ConversationRecoveryDialog({
  recovery,
  busy = false,
  onClose,
  onConfigure,
  onChooseProvider,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  useDialogFocusTrap(recovery !== null, dialogRef);

  useEffect(() => {
    if (!recovery) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [busy, onClose, recovery]);

  if (!recovery) return null;

  const unknownOwner = recovery.kind === "unknown_owner";
  const newChat = recovery.kind === "new_chat_unavailable";
  const ownerUnavailable = recovery.kind === "owner_unavailable";
  const title = unknownOwner ? "Choose a provider" : "Provider unavailable";
  const description = unknownOwner
    ? "This older chat has no saved provider. Choose one once and Zest will remember it for this chat."
    : newChat
      ? `${recovery.providerLabel} is ${recovery.configured ? "not ready" : "not configured"} for this project. Choose an available provider or configure one before opening the chat.`
      : `${recovery.providerLabel} is ${recovery.configured ? "not ready" : "not configured"} for this project. The original chat stays unchanged.`;

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/45 p-4">
      <button
        type="button"
        aria-label="Close provider choice"
        disabled={busy}
        className="absolute inset-0 cursor-default"
        onClick={onClose}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        tabIndex={-1}
        className="relative z-10 w-full max-w-[360px] overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-2xl"
      >
        <header className="flex items-start justify-between gap-3 border-b border-border/60 px-4 py-3">
          <div className="min-w-0">
            <h2 id={titleId} className="text-sm font-semibold tracking-[-0.15px]">
              {title}
            </h2>
            <p
              id={descriptionId}
              className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground"
            >
              {description}
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="Close"
            aria-label="Close"
            disabled={busy}
            onClick={onClose}
          >
            <XIcon />
          </Button>
        </header>

        <div className="p-3">
          {unknownOwner ? (
            <div className="mb-2 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
              Available providers
            </div>
          ) : (
            <div className="mb-2 rounded-lg border border-border/60 bg-card/50 px-3 py-2">
              <div className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                {newChat ? "Selected provider" : "Original provider"}
              </div>
              <div className="mt-0.5 flex items-center gap-2 text-sm font-medium">
                <span>{recovery.providerLabel}</span>
                <span className="text-[11px] text-muted-foreground">
                  {recovery.configured ? "Not ready here" : "Unavailable here"}
                </span>
              </div>
            </div>
          )}

          {recovery.providers.length ? (
            <div className="flex flex-col gap-1">
              {recovery.providers.map((provider) => (
                <button
                  key={provider.id}
                  type="button"
                  disabled={busy}
                  onClick={() => onChooseProvider(provider.id)}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left outline-none transition-colors",
                    "hover:bg-secondary/60 focus-visible:bg-secondary/60 disabled:pointer-events-none disabled:opacity-50"
                  )}
                >
                  <span className="grid size-5 shrink-0 place-items-center rounded-md bg-secondary/70 text-primary">
                    {unknownOwner ? (
                      <CheckIcon className="size-3.5" aria-hidden="true" />
                    ) : (
                      <CopyIcon className="size-3.5" aria-hidden="true" />
                    )}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-medium">
                      {ownerUnavailable
                        ? `Open a copy with ${provider.label}`
                        : `Use ${provider.label}`}
                    </span>
                    {provider.model ? (
                      <span className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground">
                        {provider.model}
                      </span>
                    ) : null}
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <p className="rounded-lg border border-border/60 bg-card/40 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
              No configured provider is ready in this project.
            </p>
          )}

          {!unknownOwner || recovery.providers.length === 0 ? (
            <>
              <div className="my-3 h-px bg-border/60" />
              <Button
                type="button"
                variant="outline"
                className="w-full justify-center"
                disabled={busy}
                onClick={onConfigure}
              >
                <Settings2Icon className="size-3.5" aria-hidden="true" />
                {unknownOwner
                  ? "Open project configuration"
                  : recovery.configured
                    ? "Choose provider or API key"
                    : `Configure ${recovery.providerLabel}`}
              </Button>
            </>
          ) : null}
        </div>
      </div>
    </div>
  );
}
