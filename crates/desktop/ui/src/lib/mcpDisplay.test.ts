import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  approvalNotice,
  approvalTitle,
  isEmptyArgsPreview,
  mcpToolLabel,
  parseMcpToolName,
  toolDisplayName,
} from "./mcpDisplay.ts";

describe("mcp display names", () => {
  it("splits a qualified MCP tool into server and tool", () => {
    assert.deepEqual(parseMcpToolName("mcp__Haiku__manifest"), {
      server: "Haiku",
      tool: "manifest",
    });
    assert.equal(mcpToolLabel("mcp__Haiku__manifest"), "Haiku · manifest");
    assert.equal(parseMcpToolName("write_file"), null);
  });

  it("asks to run the tool on the server, not to allow mcp__", () => {
    assert.equal(approvalTitle("mcp__Haiku__manifest"), "Run manifest on Haiku?");
    assert.equal(approvalTitle("bash"), "Run this command?");
    assert.equal(approvalTitle("write_file"), "Allow write_file?");
  });

  it("uses the summary for notices so mcp__ never leads", () => {
    assert.equal(
      approvalNotice("mcp__Haiku__manifest", "Run manifest on the Haiku MCP server"),
      "Run manifest on the Haiku MCP server"
    );
    assert.equal(
      approvalNotice("mcp__Haiku__manifest", "  "),
      "Haiku · manifest is waiting for your approval."
    );
  });

  it("treats empty JSON objects as no preview", () => {
    assert.equal(isEmptyArgsPreview("{}"), true);
    assert.equal(isEmptyArgsPreview(""), true);
    assert.equal(isEmptyArgsPreview('{\n  "q": "bug"\n}'), false);
  });

  it("keeps ordinary tool names readable", () => {
    assert.equal(toolDisplayName("write_file"), "write file");
    assert.equal(toolDisplayName("mcp__github__search_issues"), "github · search_issues");
  });
});
