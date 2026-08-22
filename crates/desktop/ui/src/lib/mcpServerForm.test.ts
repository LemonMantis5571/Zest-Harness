import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  messageFromError,
  parseArgs,
  parseEnvVars,
  validateMcpServerDraft,
  type McpServerDraft,
} from "./mcpServerForm.ts";

function draft(overrides: Partial<McpServerDraft> = {}): McpServerDraft {
  return {
    id: "github",
    command: "npx",
    args: "-y @modelcontextprotocol/server-github",
    envVars: "GITHUB_TOKEN",
    timeoutSecs: "120",
    ...overrides,
  };
}

describe("MCP argument parsing", () => {
  it("splits on whitespace and collapses runs", () => {
    assert.deepEqual(parseArgs("  -y   server-github "), ["-y", "server-github"]);
  });

  it("keeps a quoted path in one argument", () => {
    assert.deepEqual(parseArgs('--root "C:\\Program Files\\thing"'), [
      "--root",
      "C:\\Program Files\\thing",
    ]);
  });

  it("keeps a deliberately empty argument", () => {
    assert.deepEqual(parseArgs('--prefix "" --end'), ["--prefix", "", "--end"]);
  });

  it("is empty for a blank line", () => {
    assert.deepEqual(parseArgs("   "), []);
  });
});

describe("MCP environment variable parsing", () => {
  it("accepts commas, spaces, or both", () => {
    assert.deepEqual(parseEnvVars("A, B  C,,D"), ["A", "B", "C", "D"]);
  });

  it("is empty for a blank line", () => {
    assert.deepEqual(parseEnvVars(" , "), []);
  });
});

describe("MCP draft validation", () => {
  it("accepts a complete draft and trims it", () => {
    const result = validateMcpServerDraft(draft({ id: " github ", command: " npx " }));
    assert.equal(result.ok, true);
    assert.ok(result.ok);
    assert.equal(result.value.id, "github");
    assert.equal(result.value.command, "npx");
    assert.deepEqual(result.value.args, ["-y", "@modelcontextprotocol/server-github"]);
    assert.deepEqual(result.value.envVars, ["GITHUB_TOKEN"]);
    assert.equal(result.value.timeoutSecs, 120);
  });

  it("rejects a name the tool namespace cannot carry", () => {
    const result = validateMcpServerDraft(draft({ id: "my server!" }));
    assert.equal(result.ok, false);
    assert.ok(!result.ok);
    assert.match(result.error, /letters, numbers/);
  });

  it("rejects a missing command", () => {
    const result = validateMcpServerDraft(draft({ command: "  " }));
    assert.ok(!result.ok);
    assert.match(result.error, /command/);
  });

  /** A pasted token must never reach the config write. */
  it("rejects an environment variable given as a value", () => {
    const result = validateMcpServerDraft(draft({ envVars: "GITHUB_TOKEN=ghp_secret" }));
    assert.ok(!result.ok);
    assert.match(result.error, /names only/);
  });

  it("rejects a timeout outside the supported range", () => {
    for (const timeoutSecs of ["0", "601", "", "abc", "12.5"]) {
      const result = validateMcpServerDraft(draft({ timeoutSecs }));
      assert.ok(!result.ok, `${timeoutSecs} should be rejected`);
    }
  });
});

describe("desktop error messages", () => {
  it("unwraps the desktop error envelope", () => {
    const error = new Error(JSON.stringify({ code: "config", message: "github is not configured." }));
    assert.equal(messageFromError(error, "fallback"), "github is not configured.");
  });

  it("passes a plain message through", () => {
    assert.equal(messageFromError(new Error("boom"), "fallback"), "boom");
  });

  it("falls back when there is nothing to show", () => {
    assert.equal(messageFromError(undefined, "fallback"), "fallback");
  });
});
