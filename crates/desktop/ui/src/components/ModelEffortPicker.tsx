import { useEffect, useId, useRef, useState } from "react";
import { CheckIcon, ChevronDownIcon, RotateCcwIcon } from "lucide-react";

import {
  DEFAULT_EFFORT,
  capabilityForModel,
  chipLabel,
  effortsForModel,
  formatContextWindow,
  modelLabel,
  modelOptionsFromCapabilities,
  modelPickerGroups,
  type EffortId,
  type ModelCapability,
  type ModelPickerGroup,
} from "@/lib/models";
import type { ProviderRow } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  model: string;
  effort: EffortId;
  models?: ModelCapability[];
  defaultModel?: string;
  currentProviderId?: string;
  currentProviderLabel?: string;
  providers?: ProviderRow[];
  disabled?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  onModelChange: (model: string) => void;
  onEffortChange: (effort: EffortId) => void;
  onSwitchProvider?: (providerId: string, model: string) => void;
  /** Reset both values through one backend transaction. */
  onReset?: () => void;
};

/**
 * Plain positioned panel. Portal-based menus have been crashing the desktop
 * webview on open.
 * Model/effort availability comes from Rust; labels are display-only.
 */
export function ModelEffortPicker({
  model,
  effort,
  models,
  defaultModel,
  currentProviderId,
  currentProviderLabel,
  providers,
  disabled,
  open: openProp,
  onOpenChange,
  onModelChange,
  onEffortChange,
  onSwitchProvider,
  onReset,
}: Props) {
  const [internalOpen, setInternalOpen] = useState(false);
  const open = onOpenChange ? (openProp ?? false) : internalOpen;
  const setOpen = onOpenChange ?? setInternalOpen;
  const rootRef = useRef<HTMLDivElement>(null);
  const effortListRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const groups: ModelPickerGroup[] = currentProviderId
    ? modelPickerGroups(
        {
          providerId: currentProviderId,
          label: currentProviderLabel ?? currentProviderId,
          models,
        },
        providers ?? []
      )
    : [
        {
          providerId: currentProviderId ?? "",
          label: currentProviderLabel ?? "",
          current: true,
          models: models ?? [],
        },
      ];
  const grouped = groups.length > 1;
  const effortOptions = effortsForModel(models, model);
  const capability = capabilityForModel(models, model);
  const contextLabel = formatContextWindow(capability?.contextWindow);
  const supportsEffort = effortOptions.length > 0;
  const pickerLabel = grouped
    ? "Model and provider"
    : supportsEffort
      ? "Model and effort"
      : "Model";
  const resetModel = defaultModel ?? modelOptionsFromCapabilities(models)[0]?.id ?? model;

  useEffect(() => {
    if (!open) return;

    const onPointerDown = (event: PointerEvent) => {
      const root = rootRef.current;
      if (!root) return;
      if (event.target instanceof Node && !root.contains(event.target)) {
        setOpen(false);
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };

    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, setOpen]);

  function focusSelectedEffort() {
    requestAnimationFrame(() => {
      const selected = effortListRef.current?.querySelector("[aria-selected='true']");
      if (selected instanceof HTMLElement) selected.focus();
    });
  }

  function applyModel(
    providerId: string,
    next: string,
    groupModels: ModelCapability[]
  ) {
    const keepOpen = effortsForModel(groupModels, next).length > 0;
    if (!keepOpen) setOpen(false);
    if (currentProviderId && providerId !== currentProviderId) {
      onSwitchProvider?.(providerId, next);
      return;
    }
    if (next !== model) onModelChange(next);
    if (keepOpen) focusSelectedEffort();
  }

  function applyEffort(next: EffortId) {
    setOpen(false);
    if (next !== effort) onEffortChange(next);
  }

  function reset() {
    setOpen(false);
    if (onReset) {
      onReset();
      return;
    }
    if (resetModel !== model) onModelChange(resetModel);
    if (DEFAULT_EFFORT !== effort) onEffortChange(DEFAULT_EFFORT);
  }

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        disabled={disabled}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        title={pickerLabel}
        className={cn(
          "inline-flex min-h-8 max-w-[260px] cursor-pointer items-center gap-2 rounded-md px-2.5 py-1.5 text-xs font-medium text-foreground/85 outline-none transition-colors",
          "hover:bg-secondary/50 hover:text-foreground",
          "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-offset-1 focus-visible:ring-offset-background",
          open && "bg-secondary/60 text-foreground",
          "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50"
        )}
        onClick={() => setOpen(!open)}
      >
        <span className="truncate">{supportsEffort ? chipLabel(model, effort) : modelLabel(model)}</span>
        <ChevronDownIcon className="size-3 shrink-0 opacity-60" />
      </button>

      {open ? (
        <div
          id={panelId}
          role="dialog"
          aria-label={pickerLabel}
          className="absolute bottom-[calc(100%+6px)] left-0 z-50 w-[260px] max-h-[min(24rem,calc(100dvh-10rem))] overflow-y-auto overscroll-contain rounded-lg border border-border/60 bg-popover p-1 text-popover-foreground"
        >
          <div className="px-2 py-1.5">
            <div className="text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground">
              {grouped ? "Model and provider" : "Model"}
            </div>
            {capability ? (
              <div className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
                <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
                  {contextLabel ? <span>{contextLabel}</span> : null}
                  <span aria-hidden="true">·</span>
                  <span>{capability.supportsTools ? "Tools" : "Text only"}</span>
                  {capability.supportsVision ? <span>Vision</span> : null}
                </div>
                {!supportsEffort ? (
                  <div className="mt-0.5 text-muted-foreground/75">Reasoning is automatic</div>
                ) : null}
              </div>
            ) : null}
          </div>
          <div role="listbox" aria-label="Model" className="flex flex-col gap-0.5">
            {groups.map((group) => {
              const options = modelOptionsFromCapabilities(group.models);
              return (
                <div key={group.providerId || group.label}>
                  {grouped ? (
                    <div className="px-2 pt-1.5 pb-0.5 text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground">
                      {group.label}
                    </div>
                  ) : null}
                  {options.map((item) => {
                    const selected = group.current && item.id === model;
                    return (
                      <button
                        key={`${group.providerId}:${item.id}`}
                        type="button"
                        role="option"
                        aria-selected={selected}
                        className={cn(
                          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors",
                          "text-foreground/90 hover:bg-secondary/60 hover:text-foreground focus-visible:bg-secondary/60 focus-visible:text-foreground",
                          selected && "bg-secondary/70 text-foreground"
                        )}
                        onClick={() => applyModel(group.providerId, item.id, group.models)}
                      >
                        <span className="flex-1 truncate">{item.label}</span>
                        {selected ? <CheckIcon className="size-3.5 shrink-0" /> : null}
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>

          {supportsEffort ? (
            <>
              <div className="my-1 h-px bg-border/60" />

              <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground">
                Effort
              </div>
              <div
                ref={effortListRef}
                role="listbox"
                aria-label="Effort"
                className="flex flex-col gap-0.5"
              >
                {effortOptions.map((item) => {
                  const selected = item.id === effort;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      className={cn(
                        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors",
                        "text-foreground/90 hover:bg-secondary/60 hover:text-foreground focus-visible:bg-secondary/60 focus-visible:text-foreground",
                        selected && "bg-secondary/70 text-foreground"
                      )}
                      onClick={() => applyEffort(item.id)}
                    >
                      <span className="flex-1 truncate">{item.label}</span>
                      {selected ? <CheckIcon className="size-3.5 shrink-0" /> : null}
                    </button>
                  );
                })}
              </div>
            </>
          ) : null}

          <div className="my-1 h-px bg-border/60" />

          <button
            type="button"
            aria-label="Reset model and effort to default"
            className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm text-muted-foreground outline-none transition-colors hover:bg-secondary/60 hover:text-foreground focus-visible:bg-secondary/60 focus-visible:text-foreground"
            onClick={reset}
          >
            <RotateCcwIcon className="size-3.5 opacity-70" />
            Reset to default
          </button>
        </div>
      ) : null}
    </div>
  );
}
