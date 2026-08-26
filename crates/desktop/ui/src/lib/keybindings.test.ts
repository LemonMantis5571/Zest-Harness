import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  COMMANDS,
  chordFromEvent,
  chordIsBindable,
  commandFor,
  conflicts,
  defaultBindings,
  formatChord,
  normalizeChord,
  parseChord,
} from "./keybindings.ts";

/** The parts of a KeyboardEvent `chordFromEvent` reads. */
function keyEvent(init: {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}): KeyboardEvent {
  return {
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    ...init,
  } as KeyboardEvent;
}

describe("chord normalization", () => {
  it("spells one chord one way regardless of order or case", () => {
    // Equality is string equality everywhere else, so these must converge.
    assert.equal(normalizeChord("shift+ctrl+p"), "Mod+Shift+P");
    assert.equal(normalizeChord("Ctrl+Shift+P"), "Mod+Shift+P");
    assert.equal(normalizeChord("Cmd+Shift+p"), "Mod+Shift+P");
    assert.equal(normalizeChord("Mod+Shift+P"), "Mod+Shift+P");
  });

  it("treats Ctrl and Cmd as the same modifier", () => {
    assert.equal(normalizeChord("ctrl+n"), normalizeChord("cmd+n"));
  });

  it("keeps named keys intact", () => {
    assert.equal(normalizeChord("Mod+ArrowUp"), "Mod+ArrowUp");
    assert.equal(normalizeChord("Space"), "Space");
  });

  it("spells the space bar on the way in from an event", () => {
    // A literal " " could not survive a "+"-joined chord string, so the naming
    // has to happen at the event boundary rather than in parseChord.
    assert.equal(chordFromEvent(keyEvent({ key: " " })), "Space");
    assert.equal(chordFromEvent(keyEvent({ key: " ", ctrlKey: true })), "Mod+Space");
  });

  it("ignores a modifier pressed on its own", () => {
    // Holding Ctrl must not be treated as a chord waiting for a command.
    assert.equal(chordFromEvent(keyEvent({ key: "Control", ctrlKey: true })), null);
    assert.equal(chordFromEvent(keyEvent({ key: "Shift", shiftKey: true })), null);
  });

  it("builds the same chord from either platform modifier", () => {
    assert.equal(chordFromEvent(keyEvent({ key: "n", ctrlKey: true })), "Mod+N");
    assert.equal(chordFromEvent(keyEvent({ key: "n", metaKey: true })), "Mod+N");
  });

  it("rejects a chord with no key", () => {
    assert.equal(normalizeChord(""), null);
    assert.equal(normalizeChord("Ctrl+Shift"), null);
    assert.equal(parseChord("+++"), null);
  });
});

describe("chord display", () => {
  it("uses symbols on mac and words elsewhere", () => {
    assert.equal(formatChord("Mod+Shift+P", true), "⌘⇧P");
    assert.equal(formatChord("Mod+Shift+P", false), "Ctrl+Shift+P");
  });

  it("draws arrow keys as arrows", () => {
    assert.equal(formatChord("Mod+ArrowUp", false), "Ctrl+↑");
  });
});

describe("bindability", () => {
  it("refuses keys that would trap the user", () => {
    // Rebinding these breaks sending, focus movement, or dismissal - including
    // dismissal of the editor doing the rebinding.
    assert.equal(chordIsBindable("Enter").ok, false);
    assert.equal(chordIsBindable("Tab").ok, false);
    assert.equal(chordIsBindable("Escape").ok, false);
    assert.equal(chordIsBindable("Mod+Escape").ok, false);
  });

  it("allows a bare printable key, as `/` already is", () => {
    assert.equal(chordIsBindable("/").ok, true);
    assert.equal(chordIsBindable("Mod+Enter").ok, true);
  });
});

describe("conflicts", () => {
  it("finds nothing in the shipped defaults", () => {
    // A default set that collides with itself would be a shipped bug.
    assert.deepEqual([...conflicts(defaultBindings()).keys()], []);
  });

  it("reports every command sharing a chord", () => {
    const clashing = { ...defaultBindings(), "chat.new": "Mod+B" } as const;
    const found = conflicts(clashing);
    assert.deepEqual(found.get("Mod+B")?.sort(), ["chat.new", "view.sidebar"]);
  });

  it("ignores commands the user deliberately unbound", () => {
    const unbound = { ...defaultBindings(), "chat.new": "", "chat.stop": "" };
    assert.deepEqual([...conflicts(unbound).keys()], []);
  });
});

describe("dispatch lookup", () => {
  it("maps a chord back to its command", () => {
    const bindings = defaultBindings();
    assert.equal(commandFor(bindings, "Mod+N"), "chat.new");
    assert.equal(commandFor(bindings, "Mod+B"), "view.sidebar");
    assert.equal(commandFor(bindings, "Mod+Q"), null);
  });

  it("never matches an unbound command against the empty chord", () => {
    // The bug this guards: `bindings[id] === chord` is true for two empty
    // strings, so an unrecognised key would fire whichever command was cleared.
    const bindings = { ...defaultBindings(), "chat.new": "" };
    assert.equal(commandFor(bindings, ""), null);
  });

  it("every command has a unique id", () => {
    const ids = COMMANDS.map((c) => c.id);
    assert.equal(new Set(ids).size, ids.length);
  });
});
