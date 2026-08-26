import { useEffect, useId, useRef, useState } from "react";
import { CheckIcon, ChevronDownIcon } from "lucide-react";

import { cn } from "@/lib/utils";

type Props = {
  workerLabel: string;
  model: string;
  models: string[];
  disabled?: boolean;
  onModelChange: (model: string) => void;
};

type WorkerModelOption = {
  value: string;
  label: string;
  hint?: string;
};

function formatModelLabel(value: string): string {
  const normalized = value.trim();
  if (!normalized) return "CLI default";

  const aliases: Record<string, string> = {
    auto: "Auto",
    haiku: "Haiku",
    opus: "Opus",
    sonnet: "Sonnet",
  };
  const alias = aliases[normalized.toLowerCase()];
  if (alias) return alias;

  return normalized
    .replace(/[-_]+/g, " ")
    .replace(/\s+/g, " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function makeOption(value: string): WorkerModelOption {
  const normalized = value.trim();
  const label = formatModelLabel(normalized);

  if (!normalized) {
    return {
      value: "",
      label,
      hint: "Use the model configured in the CLI",
    };
  }

  return {
    value: normalized,
    label,
    hint: label.toLowerCase() === normalized.toLowerCase() ? undefined : normalized,
  };
}

function uniqueModels(model: string, models: string[]): string[] {
  const values = ["", ...models.map((value) => value.trim()).filter(Boolean)];
  if (model.trim() && !values.includes(model.trim())) values.push(model.trim());
  return values.filter((value, index) => values.indexOf(value) === index);
}

/**
 * Model menu for CLI workers.
 *
 * It uses an in-page menu because native select menus use the WebView's browser
 * styling instead of Zest's dark theme.
 */
export function WorkerModelPicker({
  workerLabel,
  model,
  models,
  disabled = false,
  onModelChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const menuId = useId();
  const labelId = useId();
  const normalizedModel = model.trim();
  const options = uniqueModels(normalizedModel, models).map(makeOption);
  const selectedIndex = Math.max(
    0,
    options.findIndex((option) => option.value === normalizedModel)
  );
  const selected = options[selectedIndex] ?? options[0] ?? makeOption("");
  const configuredOptions = options.filter((option) => option.value);

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
      if (event.key !== "Escape") return;
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
    };

    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      optionRefs.current[selectedIndex]?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open, selectedIndex]);

  function moveFocus(current: number, direction: 1 | -1) {
    const next = (current + direction + options.length) % options.length;
    optionRefs.current[next]?.focus();
  }

  function choose(next: string) {
    setOpen(false);
    if (next !== model) onModelChange(next);
  }

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        ref={triggerRef}
        type="button"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        aria-label={`Model used by ${workerLabel}`}
        title={selected.label}
        onClick={() => setOpen((value) => !value)}
        className={cn(
          "inline-flex min-h-8 max-w-[155px] items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-foreground/85 outline-none transition-colors",
          "hover:bg-secondary/50 hover:text-foreground",
          "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-offset-1 focus-visible:ring-offset-background",
          open && "bg-secondary/60 text-foreground",
          "disabled:pointer-events-none disabled:opacity-50"
        )}
      >
        <span className="truncate">{selected.label}</span>
        <ChevronDownIcon className="size-3 shrink-0 opacity-60" aria-hidden="true" />
      </button>

      {open ? (
        <div
          id={menuId}
          role="menu"
          aria-labelledby={labelId}
          onKeyDown={(event) => {
            const current = optionRefs.current.findIndex((item) => item === document.activeElement);
            if (event.key === "ArrowDown") {
              event.preventDefault();
              moveFocus(current < 0 ? selectedIndex : current, 1);
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              moveFocus(current < 0 ? selectedIndex : current, -1);
            } else if (event.key === "Home") {
              event.preventDefault();
              optionRefs.current[0]?.focus();
            } else if (event.key === "End") {
              event.preventDefault();
              optionRefs.current[options.length - 1]?.focus();
            }
          }}
          className="absolute bottom-[calc(100%+6px)] right-0 z-50 w-[236px] overflow-hidden rounded-lg border border-border/70 bg-popover p-1 text-popover-foreground"
        >
          <div className="px-2 py-1.5">
            <div
              id={labelId}
              className="text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground"
            >
              Worker model
            </div>
            <div className="mt-0.5 text-[10px] text-muted-foreground/75">
              {workerLabel} · independent from Zest
            </div>
          </div>

          <div className="my-0.5 h-px bg-border/60" />
          <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground/70">
            CLI setting
          </div>
          <button
            ref={(element) => {
              optionRefs.current[0] = element;
            }}
            type="button"
            role="menuitemradio"
            aria-checked={selected.value === ""}
            onClick={() => choose("")}
            className={cn(
              "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left outline-none transition-colors",
              "hover:bg-secondary/60 focus-visible:bg-secondary/60",
              selected.value === "" && "bg-secondary/70"
            )}
          >
            <span className="grid size-3.5 shrink-0 place-items-center">
              {selected?.value === "" ? <CheckIcon className="size-3.5" /> : null}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[12px] text-foreground/90">CLI default</span>
              <span className="block truncate text-[10px] leading-snug text-muted-foreground/75">
                Use the model configured in the CLI
              </span>
            </span>
          </button>

          {configuredOptions.length ? (
            <>
              <div className="my-0.5 h-px bg-border/60" />
              <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-[0.04em] text-muted-foreground/70">
                Available models
              </div>
              {configuredOptions.map((option, index) => {
                const optionIndex = index + 1;
                const active = option.value === normalizedModel;
                return (
                  <button
                    key={option.value}
                    ref={(element) => {
                      optionRefs.current[optionIndex] = element;
                    }}
                    type="button"
                    role="menuitemradio"
                    aria-checked={active}
                    onClick={() => choose(option.value)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left outline-none transition-colors",
                      "hover:bg-secondary/60 focus-visible:bg-secondary/60",
                      active && "bg-secondary/70"
                    )}
                  >
                    <span className="grid size-3.5 shrink-0 place-items-center">
                      {active ? <CheckIcon className="size-3.5" /> : null}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[12px] text-foreground/90">
                        {option.label}
                      </span>
                      {option.hint ? (
                        <span className="block truncate font-mono text-[10px] leading-snug text-muted-foreground/70">
                          {option.hint}
                        </span>
                      ) : null}
                    </span>
                  </button>
                );
              })}
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
