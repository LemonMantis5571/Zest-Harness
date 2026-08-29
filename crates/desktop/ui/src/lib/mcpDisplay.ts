/**
 * Human labels for Zest-owned MCP tools.
 *
 * The model sees `mcp__Haiku__manifest`. People should see `Haiku · manifest`.
 */

export type McpToolRef = {
  server: string;
  tool: string;
};

export function parseMcpToolName(name: string): McpToolRef | null {
  if (!name.startsWith("mcp__")) return null;
  const rest = name.slice("mcp__".length);
  const split = rest.indexOf("__");
  if (split <= 0) return null;
  const server = rest.slice(0, split);
  const tool = rest.slice(split + 2);
  if (!server || !tool) return null;
  return { server, tool };
}

export function mcpToolLabel(name: string): string | null {
  const parsed = parseMcpToolName(name);
  if (!parsed) return null;
  return `${parsed.server} · ${parsed.tool}`;
}

export function toolDisplayName(name: string): string {
  return mcpToolLabel(name) ?? name.replaceAll("_", " ");
}

export function approvalTitle(name: string): string {
  const parsed = parseMcpToolName(name);
  if (parsed) return `Run ${parsed.tool} on ${parsed.server}?`;
  if (name === "bash") return "Run this command?";
  if (name === "delegate_external") return "Delegate this task?";
  return `Allow ${name}?`;
}

/** Toast / notification body. Prefer the backend summary; never lead with mcp__. */
export function approvalNotice(toolName: string, summary?: string): string {
  const trimmed = summary?.trim();
  if (trimmed) return trimmed;
  return `${toolDisplayName(toolName)} is waiting for your approval.`;
}

export function isEmptyArgsPreview(diff: string | undefined): boolean {
  const trimmed = diff?.trim() ?? "";
  return trimmed.length === 0 || trimmed === "{}" || trimmed === "{ }";
}
