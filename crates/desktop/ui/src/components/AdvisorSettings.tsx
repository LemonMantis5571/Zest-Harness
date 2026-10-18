import { CheckIcon } from "lucide-react";
import type { ReactNode } from "react";

import { modelLabel } from "@/lib/models";
import type { AdvisorChoice, AdvisorSettings as AdvisorSettingsData } from "@/lib/types";
import { cn } from "@/lib/utils";

const USE_COUNTS = [1, 2, 3, 5] as const;

type Props = {
  settings: AdvisorSettingsData | null;
  loading: boolean;
  /** A save is in flight; every control waits for it. */
  saving: boolean;
  error: string | null;
  chatProviderId: string;
  chatProviderLabel: string;
  /** The chat's provider runs its own tools, so it cannot call the advisor. */
  chatOwnsAgentLoop: boolean;
  sending: boolean;
  onSave: (choice: AdvisorChoice | null) => void;
};

/**
 * Choose the stronger model the chat may consult mid-turn.
 *
 * Every choice saves at once, like the other switches in Settings: there is no
 * draft state to lose, and the session is rebuilt so the open chat uses it.
 * Option buttons rather than a native select, which the WebView draws in its
 * own light theme.
 */
export function AdvisorSettings({
  settings,
  loading,
  saving,
  error,
  chatProviderId,
  chatProviderLabel,
  chatOwnsAgentLoop,
  sending,
  onSave,
}: Props) {
  if (loading && !settings) {
    return (
      <p className="m-0 text-[11px] text-muted-foreground" role="status">
        Loading advisor settings…
      </p>
    );
  }
  if (!settings) {
    return (
      <p className="m-0 text-[11px] text-destructive" role="alert">
        {error ?? "Could not load advisor settings. Try again."}
      </p>
    );
  }

  const disabled = saving || sending;
  const current = settings.options.find((option) => option.id === settings.provider) ?? null;
  const model = settings.model || current?.defaultModel || "";
  const crossProvider = Boolean(current) && settings.provider !== chatProviderId;

  // A newly chosen provider starts on its own default model.
  const choose = (provider: string) => {
    const option = settings.options.find((candidate) => candidate.id === provider);
    onSave({
      provider,
      model: option?.defaultModel || null,
      maxUses: settings.maxUses,
      autoReview: settings.autoReview,
    });
  };
  const update = (patch: { model?: string; maxUses?: number; autoReview?: boolean }) => {
    if (!current) return;
    onSave({
      provider: current.id,
      model: patch.model ?? (settings.model || null),
      maxUses: patch.maxUses ?? settings.maxUses,
      autoReview: patch.autoReview ?? settings.autoReview,
    });
  };

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
        A stronger model this chat can ask for advice mid-task: before committing to an approach,
        when an error keeps coming back, or before finishing. It reads the conversation and cannot
        run tools or change files.
      </p>

      <Group label="Advisor">
        <Option
          selected={!settings.provider}
          disabled={disabled}
          title="Off"
          onSelect={() => {
            if (settings.provider) onSave(null);
          }}
        />
        {settings.options.map((option) => (
          <Option
            key={option.id}
            selected={option.id === settings.provider}
            disabled={disabled || !option.ready}
            title={option.label}
            detail={
              option.id === chatProviderId
                ? `${option.statusLabel} · same as this chat`
                : option.statusLabel
            }
            onSelect={() => {
              if (option.id !== settings.provider) choose(option.id);
            }}
          />
        ))}
      </Group>
      {settings.options.length === 0 ? (
        <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
          No API provider is set up yet. Connect one (an API key, an OpenAI-compatible endpoint, or
          ChatGPT sign-in) to use it as the advisor.
        </p>
      ) : null}

      {current && current.models.length > 1 ? (
        <Group label="Advisor model" compact>
          {current.models.map((id) => (
            <Chip
              key={id}
              selected={id === model}
              disabled={disabled}
              onSelect={() => {
                if (id !== model) update({ model: id });
              }}
            >
              {modelLabel(id)}
            </Chip>
          ))}
        </Group>
      ) : null}

      {current ? (
        <Group label="Consultations per request" compact>
          {USE_COUNTS.map((count) => (
            <Chip
              key={count}
              selected={count === settings.maxUses}
              disabled={disabled}
              onSelect={() => {
                if (count !== settings.maxUses) update({ maxUses: count });
              }}
            >
              {count}
            </Chip>
          ))}
        </Group>
      ) : null}

      {current ? (
        <div className="flex items-start justify-between gap-3 rounded-lg border border-border/80 bg-card/70 px-3 py-2.5">
          <div className="min-w-0">
            <div className="text-sm font-medium">Review automatically</div>
            <p className="m-0 mt-1 text-[11px] leading-relaxed text-muted-foreground">
              Zest also asks the advisor itself: before a turn that changed files ends, and after
              two failed commands in a row. Counts toward the consultations above.
            </p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={settings.autoReview}
            aria-label="Review automatically"
            disabled={disabled}
            onClick={() => update({ autoReview: !settings.autoReview })}
            className={cn(
              "relative mt-0.5 inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50",
              settings.autoReview ? "bg-primary" : "bg-muted"
            )}
          >
            <span
              className={cn(
                "pointer-events-none size-4 rounded-full bg-background shadow-sm transition-transform",
                settings.autoReview ? "translate-x-4" : "translate-x-0"
              )}
            />
          </button>
        </div>
      ) : null}

      {current && chatOwnsAgentLoop ? (
        <Note>
          This chat runs on {chatProviderLabel}, which uses its own tools, so it cannot consult the
          advisor. Chats on an API provider can.
        </Note>
      ) : current && crossProvider ? (
        <Note>
          This chat runs on {chatProviderLabel}. The first time each session, Zest asks before
          sending the conversation to {current.label}.
        </Note>
      ) : null}

      {settings.issue ? (
        <p className="m-0 text-[11px] leading-relaxed text-destructive" role="alert">
          {settings.issue}
        </p>
      ) : null}
      {error ? (
        <p className="m-0 text-[11px] leading-relaxed text-destructive" role="alert">
          {error}
        </p>
      ) : null}
      <p className="m-0 text-[10px] leading-relaxed text-muted-foreground/80" role="status">
        {saving ? "Saving…" : `Saved in ${settings.scope}.`}
      </p>
    </div>
  );
}

function Group({
  label,
  compact = false,
  children,
}: {
  label: string;
  compact?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-[11px] font-medium text-muted-foreground">{label}</span>
      <div
        role="radiogroup"
        aria-label={label}
        className={compact ? "flex flex-wrap gap-1.5" : "flex flex-col gap-1.5"}
      >
        {children}
      </div>
    </div>
  );
}

function Option({
  selected,
  disabled,
  title,
  detail,
  onSelect,
}: {
  selected: boolean;
  disabled: boolean;
  title: string;
  detail?: string;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        "flex w-full items-center justify-between gap-2 rounded-lg border px-3 py-2 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50",
        "disabled:cursor-not-allowed disabled:opacity-50",
        selected
          ? "border-primary/70 bg-primary/10"
          : "border-border/80 bg-card/70 hover:border-border hover:bg-card"
      )}
    >
      <span className="min-w-0">
        <span className="block truncate text-sm font-medium">{title}</span>
        {detail ? (
          <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">{detail}</span>
        ) : null}
      </span>
      {selected ? (
        <span className="flex size-4 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground">
          <CheckIcon className="size-2.5 stroke-[3]" aria-hidden="true" />
        </span>
      ) : null}
    </button>
  );
}

function Chip({
  selected,
  disabled,
  onSelect,
  children,
}: {
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        "min-h-7 rounded-md border px-2.5 text-xs font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50",
        "disabled:cursor-not-allowed disabled:opacity-50",
        selected
          ? "border-primary/70 bg-primary/15 text-foreground"
          : "border-border/80 text-muted-foreground hover:border-border hover:text-foreground"
      )}
    >
      {children}
    </button>
  );
}

function Note({ children }: { children: ReactNode }) {
  return (
    <p className="m-0 rounded-lg border border-border/70 bg-card/50 px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
      {children}
    </p>
  );
}
