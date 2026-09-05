import { useEffect, useId, useRef, useState } from "react";
import { CheckIcon, ChevronDownIcon, RotateCcwIcon } from "lucide-react";

import {
  DEFAULT_EFFORT,
  capabilityForModel,
  chipLabel,
  effortsForModel,
  formatContextWindow,
  filterModelPickerGroups,
  modelLabel,
  modelOptionsFromCapabilities,
  modelPickerGroups,
  type EffortId,
  type ModelCapability,
  type ModelPickerGroup,
} from "@/lib/models";
import type { ProviderRow } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useOptionNavigation } from "@/lib/useOptionNavigation";

type Props = {
  model: string;
  effort: EffortId;
  models?: ModelCapability[];
  defaultModel?: string;
  currentProviderId?: string;
  currentProviderLabel?: string;
  providers?: ProviderRow[];
  disabled?: boolean;
  pending?: boolean;
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
  pending = false,
  open: openProp,
  onOpenChange,
  onModelChange,
  onEffortChange,
  onSwitchProvider,
  onReset,
}: Props) {
  const [internalOpen, setInternalOpen] = useState(false);
  const [query, setQuery] = useState("");
  const open = onOpenChange ? (openProp ?? false) : internalOpen;
  const setOpen = onOpenChange ?? setInternalOpen;
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const focusEffortAfterSave = useRef(false);
  const restoreTriggerAfterSave = useRef(false);
  const searchRef = useRef<HTMLInputElement>(null);
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
  const visibleGroups = filterModelPickerGroups(groups, query);
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
  const modelNavigation = useOptionNavigation(
    visibleGroups.flatMap((group) => group.models.map((item) => `${group.providerId}:${item.id}`)),
    `${currentProviderId ?? ""}:${model}`,
    disabled,
  );
  const effortNavigation = useOptionNavigation(effortOptions.map((item) => item.id), effort, disabled, "horizontal");

  useEffect(() => {
    if (!open) return;
    searchRef.current?.focus();
  }, [open]);

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
      if (event.key === "Escape") {
        setOpen(false);
        restoreTriggerAfterSave.current = !!triggerRef.current?.disabled;
        triggerRef.current?.focus();
      }
    };

    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, setOpen]);

  useEffect(() => {
    if (!open) focusEffortAfterSave.current = false;
    if (open && !disabled && focusEffortAfterSave.current) {
      focusEffortAfterSave.current = false;
      focusSelectedEffort();
    }
    if (!open && !disabled && restoreTriggerAfterSave.current) {
      restoreTriggerAfterSave.current = false;
      if (document.activeElement === document.body) triggerRef.current?.focus();
    }
  }, [open, disabled]);

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
    if (disabled) return;
    const keepOpen = effortsForModel(groupModels, next).length > 0;
    if (!keepOpen) {
      restoreTriggerAfterSave.current = true;
      setOpen(false);
      triggerRef.current?.focus();
    }
    focusEffortAfterSave.current = keepOpen;
    if (currentProviderId && providerId !== currentProviderId) {
      onSwitchProvider?.(providerId, next);
      return;
    }
    if (next !== model) onModelChange(next);
    if (keepOpen && next === model) {
      focusEffortAfterSave.current = false;
      focusSelectedEffort();
    }
  }

  function applyEffort(next: EffortId) {
    if (disabled) return;
    restoreTriggerAfterSave.current = true;
    setOpen(false);
    triggerRef.current?.focus();
    if (next !== effort) onEffortChange(next);
  }

  function reset() {
    if (disabled) return;
    restoreTriggerAfterSave.current = true;
    setOpen(false);
    triggerRef.current?.focus();
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
        ref={triggerRef}
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
          aria-busy={pending}
          className="absolute bottom-[calc(100%+6px)] left-0 z-50 flex w-[340px] max-w-[calc(100vw-2rem)] max-h-[min(30rem,calc(100dvh-10rem))] flex-col overflow-hidden rounded-lg border border-border/60 bg-popover p-1 text-popover-foreground"
        >
          {pending ? <div role="status" className="px-2 py-1 text-xs text-muted-foreground">Saving selection…</div> : null}
          <div className="shrink-0 px-2 py-1.5">
            <div className="text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground">
              {grouped ? "Model and provider" : "Model"}
            </div>
            <div className="mt-1 truncate text-xs font-medium" title={`${currentProviderLabel ?? ""} · ${modelLabel(model)}`}>
              {currentProviderLabel ? `${currentProviderLabel} · ` : ""}{modelLabel(model)}
            </div>
            {capability ? (
              <div className="mt-1 text-[10px] leading-relaxed text-muted-foreground">
                <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
                  {contextLabel ? <span>{contextLabel}</span> : null}
                  <span>{capability.supportsTools ? "Tools" : "Text only"}</span>
                  {capability.supportsVision ? <span>Vision</span> : null}
                </div>
                {!supportsEffort ? (
                  <div className="mt-0.5 text-muted-foreground/75">No effort control</div>
                ) : null}
              </div>
            ) : null}
          </div>
          <div className="mx-1 mb-1 flex shrink-0 items-center gap-1">
            <input
              ref={searchRef}
              type="search"
              aria-label="Search models and providers"
              placeholder="Search models and providers"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  modelNavigation.focus(modelNavigation.activeKey);
                }
              }}
              className="min-w-0 flex-1 rounded-md border border-border/70 bg-background px-2 py-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            />
            {query ? <button type="button" className="rounded px-2 py-1 text-xs hover:bg-secondary/60" onClick={() => { setQuery(""); searchRef.current?.focus(); }}>Clear</button> : null}
          </div>
          <div role="listbox" aria-label="Model" className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto overscroll-contain">
            {visibleGroups.map((group) => {
              const options = group.models;
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
                        disabled={disabled}
                        {...modelNavigation.optionProps(`${group.providerId}:${item.id}`)}
                        role="option"
                        aria-selected={selected}
                        title={modelLabel(item.id)}
                        className={cn(
                          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors",
                          "text-foreground/90 hover:bg-secondary/60 hover:text-foreground focus-visible:bg-secondary/60 focus-visible:text-foreground",
                          selected && "bg-secondary/70 text-foreground"
                        )}
                        onClick={() => applyModel(group.providerId, item.id, group.models)}
                      >
                        <span className="min-w-0 flex-1">
                          <span className="block truncate">{modelLabel(item.id)}</span>
                          <span className="mt-0.5 flex flex-wrap gap-x-2 text-[10px] text-muted-foreground">
                            {formatContextWindow(item.contextWindow) ? <span>{formatContextWindow(item.contextWindow)}</span> : null}
                            <span>{item.supportsTools ? "Tools" : "Text only"}</span>
                            {item.supportsVision ? <span>Vision</span> : null}
                            {item.efforts.length === 0 ? <span>No effort control</span> : null}
                          </span>
                        </span>
                        {selected ? <CheckIcon className="size-3.5 shrink-0" /> : null}
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>
          {visibleGroups.every((group) => group.models.length === 0) ? <div role="status" className="px-2 py-4 text-xs text-muted-foreground">No models match your search.</div> : null}

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
                aria-orientation="horizontal"
                className="flex shrink-0 gap-0.5"
              >
                {effortOptions.map((item) => {
                  const selected = item.id === effort;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      disabled={disabled}
                      {...effortNavigation.optionProps(item.id)}
                      role="option"
                      aria-selected={selected}
                      className={cn(
                        "flex min-w-0 flex-1 items-center justify-center rounded-md px-1 py-1.5 text-center text-xs outline-none transition-colors",
                        "text-foreground/90 hover:bg-secondary/60 hover:text-foreground focus-visible:bg-secondary/60 focus-visible:text-foreground",
                        selected && "bg-secondary/70 text-foreground"
                      )}
                      onClick={() => applyEffort(item.id)}
                    >
                      <span>{item.label}</span>
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
            disabled={disabled}
            className="flex w-full shrink-0 items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm text-muted-foreground outline-none transition-colors hover:bg-secondary/60 hover:text-foreground focus-visible:bg-secondary/60 focus-visible:text-foreground disabled:opacity-50"
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
