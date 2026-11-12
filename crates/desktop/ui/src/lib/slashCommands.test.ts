import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  btwQuestion,
  filterSlashCommands,
  isModelSlash,
  slashTokenAt,
  splitSlashMatch,
} from "./slashCommands.ts";
import type { CommandView } from "./types.ts";

describe("temporary side question command", () => {
  it("recognizes a complete leading /btw, including an empty question", () => {
    assert.equal(btwQuestion("/btw"), "");
    assert.equal(btwQuestion("  /BTW\n¿Por qué?  "), "¿Por qué?");
    assert.equal(btwQuestion("/btw first\nsecond"), "first\nsecond");
  });
  it("preserves paths, escaped commands, and inline mentions", () => {
    for (const text of ["//btw", "/btw/file", "/btw-next", "explain /btw", "/btw?"]) {
      assert.equal(btwQuestion(text), null, text);
    }
  });
});

const commands: CommandView[] = [
  { name: "plan", description: "Write a plan", kind: "skill" },
  { name: "Haiku", description: "Use the Haiku MCP server", kind: "mcp" },
  { name: "github", description: "Use the github MCP server", kind: "mcp" },
];

describe("slash command matching", () => {
  it("matches an MCP server by prefix, case-insensitive", () => {
    const hits = filterSlashCommands(commands, "hai");
    assert.deepEqual(
      hits.map((item) => item.name),
      ["Haiku"]
    );
  });

  it("matches github from /git", () => {
    const hits = filterSlashCommands(commands, "git");
    assert.equal(hits.length, 1);
    assert.equal(hits[0]?.kind, "mcp");
  });

  it("treats only a leading /model token as the builtin", () => {
    assert.equal(isModelSlash("/model"), true);
    assert.equal(isModelSlash("  /model luna"), true);
    assert.equal(isModelSlash("please /model"), false);
    assert.equal(isModelSlash("/plan"), false);
  });

  it("highlights the typed prefix", () => {
    assert.deepEqual(splitSlashMatch("Haiku", "hai"), {
      prefix: "",
      match: "Hai",
      suffix: "ku",
    });
    assert.deepEqual(splitSlashMatch("supabase", "supa"), {
      prefix: "",
      match: "supa",
      suffix: "base",
    });
  });

  it("finds the command token at the caret anywhere in a draft", () => {
    const draft = "review this /plan then /hai";
    assert.deepEqual(slashTokenAt(draft, draft.length), {
      query: "hai",
      start: draft.lastIndexOf("/hai"),
      end: draft.length,
    });
    assert.deepEqual(slashTokenAt(draft, draft.indexOf("/plan") + "/plan".length), {
      query: "plan",
      start: draft.indexOf("/plan"),
      end: draft.indexOf("/plan") + "/plan".length,
    });
  });

  it("opens for an empty token but ignores paths and embedded slashes", () => {
    const draft = "look at /etc/hosts and ";
    assert.deepEqual(slashTokenAt(`${draft}/`, `${draft}/`.length), {
      query: "",
      start: draft.length,
      end: draft.length + 1,
    });
    assert.equal(slashTokenAt("/etc/hosts", "/etc/hosts".length), null);
    assert.equal(slashTokenAt("hello/plan", "hello/plan".length), null);
  });
});
