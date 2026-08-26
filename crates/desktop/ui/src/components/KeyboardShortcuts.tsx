import { RotateCcwIcon, XIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  COMMANDS,
  SECTIONS,
  chordFromEvent,
  chordIsBindable,
  commandById,
  conflicts,
  defaultBindings,
  formatChord,
  isMac,
  parseChord,
  type CommandId,
} from "@/lib/keybindings";
import { useBindings } from "@/lib/useKeybindings";
import { cn, scrollBehavior } from "@/lib/utils";

/**
 * Rebind commands, and show which keys are already spoken for.
 *
 * The layout is the point: a list alone answers "what does Ctrl+B do" but not
 * "which keys are free", which is the question you have when inventing a
 * shortcut.
 */
export function KeyboardShortcuts() {
  const [bindings, setBindings] = useBindings();
  const [recording, setRecording] = useState<CommandId | null>(null);
  const [rejected, setRejected] = useState<string | null>(null);

  const clashes = useMemo(() => conflicts(bindings), [bindings]);
  const mac = isMac();

  // While recording, the whole keyboard belongs to this row. Capture phase and
  // stopPropagation so the app's own dispatcher does not also fire the chord
  // being assigned - otherwise binding Ctrl+N opens a new chat mid-edit.
  useEffect(() => {
    if (!recording) return;

    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();

      if (event.key === "Escape") {
        setRecording(null);
        setRejected(null);
        return;
      }

      const chord = chordFromEvent(event);
      if (!chord) return;

      const verdict = chordIsBindable(chord);
      if (!verdict.ok) {
        setRejected(verdict.reason);
        return;
      }

      setBindings({ ...bindings, [recording]: chord });
      setRecording(null);
      setRejected(null);
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [recording, bindings, setBindings]);

  const boundKeys = useMemo(() => {
    const keys = new Set<string>();
    for (const command of COMMANDS) {
      const chord = bindings[command.id];
      if (!chord) continue;
      const parsed = parseChord(chord);
      if (parsed) keys.add(parsed.key.toUpperCase());
    }
    return keys;
  }, [bindings]);

  const changed = COMMANDS.some((c) => bindings[c.id] !== c.defaultChord);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-start justify-between gap-3">
        <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
          Click a shortcut, then press the keys. {mac ? "⌘" : "Ctrl"} and{" "}
          {mac ? "⌘" : "Ctrl"} are interchangeable across platforms. Escape cancels recording
          and is not itself rebindable — it always dismisses whatever is open.
        </p>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          disabled={!changed}
          onClick={() => setBindings(defaultBindings())}
          title="Restore every default"
        >
          <RotateCcwIcon className="size-3.5" />
          Reset all
        </Button>
      </div>

      {rejected ? (
        <p className="m-0 rounded-md border border-destructive/40 bg-destructive/5 px-2.5 py-1.5 text-[11px] text-destructive">
          {rejected}
        </p>
      ) : null}

      <KeyboardLayout boundKeys={boundKeys} />

      {SECTIONS.map((section) => {
        const rows = COMMANDS.filter((c) => c.section === section);
        if (rows.length === 0) return null;
        return (
          <div key={section} className="flex flex-col gap-1">
            <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
              {section}
            </div>
            <ul className="m-0 list-none rounded-lg border border-border/60 p-0">
              {rows.map((command) => {
                const chord = bindings[command.id] ?? "";
                const clash = chord ? clashes.get(chord) : undefined;
                return (
                  <ShortcutRow
                    key={command.id}
                    label={command.label}
                    hint={command.hint}
                    chord={chord}
                    mac={mac}
                    recording={recording === command.id}
                    isDefault={chord === command.defaultChord}
                    clashesWith={clash?.filter((id) => id !== command.id) ?? []}
                    onRecord={() => {
                      setRejected(null);
                      setRecording(recording === command.id ? null : command.id);
                    }}
                    onClear={() => setBindings({ ...bindings, [command.id]: "" })}
                    onReset={() =>
                      setBindings({ ...bindings, [command.id]: command.defaultChord })
                    }
                  />
                );
              })}
            </ul>
          </div>
        );
      })}
    </div>
  );
}

function ShortcutRow({
  label,
  hint,
  chord,
  mac,
  recording,
  isDefault,
  clashesWith,
  onRecord,
  onClear,
  onReset,
}: {
  label: string;
  hint?: string;
  chord: string;
  mac: boolean;
  recording: boolean;
  isDefault: boolean;
  clashesWith: CommandId[];
  onRecord: () => void;
  onClear: () => void;
  onReset: () => void;
}) {
  const clashLabels = clashesWith
    .map((id) => commandById(id)?.label)
    .filter((l): l is string => Boolean(l));

  return (
    <li className="flex items-center gap-3 border-b border-border/40 px-3 py-2 last:border-b-0">
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[12px]">{label}</span>
        {clashLabels.length > 0 ? (
          <span className="mt-0.5 block text-[11px] text-amber-400">
            Also runs {clashLabels.join(", ")}
          </span>
        ) : hint ? (
          <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">{hint}</span>
        ) : null}
      </span>

      <button
        type="button"
        onClick={onRecord}
        aria-label={
          recording
            ? `Recording a new shortcut for ${label}. Press keys, or Escape to cancel.`
            : chord
              ? `${label}: ${formatChord(chord, false)}. Click to change.`
              : `${label}: not bound. Click to set.`
        }
        className={cn(
          "min-w-[92px] cursor-pointer rounded-md border px-2 py-1 font-mono text-[11px] transition-colors",
          recording
            ? "border-primary/60 bg-primary/10 text-foreground"
            : "border-border/70 text-muted-foreground hover:border-border hover:text-foreground",
          clashLabels.length > 0 && !recording && "border-amber-500/50 text-amber-400"
        )}
      >
        {recording ? "Press keys…" : chord ? formatChord(chord, mac) : "Unbound"}
      </button>

      <span className="flex w-[52px] shrink-0 justify-end gap-0.5">
        {chord ? (
          <button
            type="button"
            onClick={onClear}
            title="Clear this shortcut"
            aria-label={`Clear the shortcut for ${label}`}
            className="cursor-pointer rounded p-1 text-muted-foreground/70 transition-colors hover:text-foreground"
          >
            <XIcon className="size-3" />
          </button>
        ) : null}
        {!isDefault ? (
          <button
            type="button"
            onClick={onReset}
            title="Restore the default"
            aria-label={`Restore the default shortcut for ${label}`}
            className="cursor-pointer rounded p-1 text-muted-foreground/70 transition-colors hover:text-foreground"
          >
            <RotateCcwIcon className="size-3" />
          </button>
        ) : null}
      </span>
    </li>
  );
}

/** Rows are physical-layout order, not alphabetical — this is a picture of a keyboard. */
const ROWS: string[][] = [
  ["`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "="],
  ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "[", "]"],
  ["A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'"],
  ["Z", "X", "C", "V", "B", "N", "M", ",", ".", "/"],
];

/** Widest row drives the column count so shorter rows stay centered and scale. */
const KEYBOARD_COLS = ROWS[0].length;

function KeyboardLayout({ boundKeys }: { boundKeys: Set<string> }) {
  const bound = [...boundKeys].filter((k) => k.length === 1).sort();

  return (
    <div
      className="@container w-full min-w-0 rounded-lg border border-border/60 bg-card/40 p-2.5"
      role="img"
      aria-label={
        bound.length
          ? `Keyboard layout. Keys in use: ${bound.join(", ")}.`
          : "Keyboard layout. No single-character keys are in use."
      }
    >
      <div className="flex w-full min-w-0 flex-col gap-0.5">
        {ROWS.map((row, index) => {
          const offset = Math.floor((KEYBOARD_COLS - row.length) / 2);
          return (
            <div
              key={index}
              className="grid w-full min-w-0 gap-0.5"
              style={{
                gridTemplateColumns: `repeat(${KEYBOARD_COLS}, minmax(0, 1fr))`,
              }}
            >
              {Array.from({ length: offset }, (_, i) => (
                <span key={`pad-l-${i}`} aria-hidden className="min-w-0" />
              ))}
              {row.map((key) => {
                const used = boundKeys.has(key.toUpperCase());
                return (
                  <span
                    key={key}
                    title={used ? `${key} is used by a shortcut` : undefined}
                    className={cn(
                      "flex aspect-square min-w-0 items-center justify-center rounded-[3px] border font-mono text-[clamp(8px,2.6cqw,10px)] transition-colors",
                      used
                        ? "border-primary/50 bg-primary/20 text-foreground"
                        : "border-border/50 bg-background/40 text-muted-foreground/50"
                    )}
                  >
                    {key}
                  </span>
                );
              })}
              {Array.from(
                { length: KEYBOARD_COLS - row.length - offset },
                (_, i) => (
                  <span key={`pad-r-${i}`} aria-hidden className="min-w-0" />
                )
              )}
            </div>
          );
        })}
      </div>
      <p className="m-0 mt-1.5 text-[10px] text-muted-foreground">
        Highlighted keys are used by a shortcut, with or without modifiers.
      </p>
    </div>
  );
}

/** Scroll a section into view when the caller bumps a counter. */
export function useScrollIntoViewOnBump(bump: number) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (bump <= 0) return;
    // After the Collapsible has opened, or the target is still zero-height.
    const id = requestAnimationFrame(() =>
      ref.current?.scrollIntoView({ behavior: scrollBehavior(), block: "start" })
    );
    return () => cancelAnimationFrame(id);
  }, [bump]);
  return ref;
}
