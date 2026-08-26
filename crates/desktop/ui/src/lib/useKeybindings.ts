import { useCallback, useEffect, useRef, useState } from "react";

import {
  chordFromEvent,
  commandFor,
  loadBindings,
  parseChord,
  saveBindings,
  type Bindings,
  type CommandId,
} from "./keybindings";

/** Broadcast so every mounted consumer re-reads after the editor saves. */
const CHANGED = "zest:keybindings-changed";

/** The current bindings, kept in step with edits made anywhere in the app. */
export function useBindings(): [Bindings, (next: Bindings) => void] {
  const [bindings, setBindings] = useState<Bindings>(() => loadBindings());

  useEffect(() => {
    const reload = () => setBindings(loadBindings());
    window.addEventListener(CHANGED, reload);
    // `storage` only fires in *other* windows, which is exactly what a second
    // Zest window needs to stay consistent.
    window.addEventListener("storage", reload);
    return () => {
      window.removeEventListener(CHANGED, reload);
      window.removeEventListener("storage", reload);
    };
  }, []);

  const update = useCallback((next: Bindings) => {
    saveBindings(next);
    setBindings(next);
    window.dispatchEvent(new Event(CHANGED));
  }, []);

  return [bindings, update];
}

export type CommandHandlers = Partial<Record<CommandId, () => void>>;

/**
 * Run the handler bound to whatever the user pressed.
 *
 * Escape is deliberately absent: it means "dismiss the thing on top", which
 * depends on what is currently open, so it stays with the surfaces that own
 * that context rather than becoming a rebindable global.
 */
export function useKeybindings(handlers: CommandHandlers, enabled = true): void {
  const [bindings] = useBindings();

  // Held in a ref so a new handler identity each render does not re-subscribe.
  const latest = useRef(handlers);
  latest.current = handlers;

  useEffect(() => {
    if (!enabled) return;

    const onKeyDown = (event: KeyboardEvent) => {
      const chord = chordFromEvent(event);
      if (!chord) return;

      // An unmodified key belongs to whatever the user is typing into. Without
      // this, `/` would steal every slash typed in the composer.
      const parsed = parseChord(chord);
      if (parsed && parsed.mods.length === 0 && isTypingTarget(event.target)) return;

      const id = commandFor(bindings, chord);
      if (!id) return;
      const handler = latest.current[id];
      if (!handler) return;

      event.preventDefault();
      handler();
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [bindings, enabled]);
}

function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable
  );
}
