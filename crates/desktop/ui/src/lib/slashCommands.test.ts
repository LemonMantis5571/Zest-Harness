import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  filterSlashCommands,
  isModelSlash,
  splitSlashMatch,
} from "./slashCommands.ts";
import type { CommandView } from "./types.ts";

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
});
