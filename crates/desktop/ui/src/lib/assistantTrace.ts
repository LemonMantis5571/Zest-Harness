import { htmlDocumentFromMetadata } from "./htmlArtifacts.ts";
import { stepDuration } from "./subagents.ts";
import { COLLAPSE_THRESHOLD, summarizeTools, type ToolRunSummary } from "./toolRuns.ts";
import type { ChatMessage, ProviderActivityPart, ToolPart } from "./types.ts";

type AssistantMessage = Extract<ChatMessage, { role: "assistant" }>;

/** Live UI state only; the persisted message and event formats do not change. */
export type TraceMessage = AssistantMessage & {
  /** Reply length when the latest thinking delta arrived. */
  thinkingTextLength?: number;
};

/** Duration shown by a subagent row. */
export function subagentDuration(
  part: Pick<ToolPart, "status" | "startedAt" | "finishedAt">,
  now: number
): string | undefined {
  if (part.status !== "running" && part.finishedAt == null) return undefined;
  return stepDuration(part.startedAt, part.finishedAt, now);
}

/** One thing the turn did, resolved from a step reference to its part. */
export type TraceStep =
  | { kind: "thinking"; key: string; text: string; live: boolean }
  | { kind: "tool"; tool: ToolPart }
  | { kind: "activity"; activity: ProviderActivityPart };

/** A provider activity with the activities that ran inside it. */
export type ActivityNode = {
  activity: ProviderActivityPart;
  children: ActivityNode[];
};

/** One line of the turn's `Trace`. */
export type TraceItem =
  | { kind: "thinking"; key: string; text: string; live: boolean }
  | { kind: "tool"; key: string; tool: ToolPart }
  | { kind: "tool-group"; key: string; tools: ToolPart[]; summary: ToolRunSummary }
  | { kind: "activity"; key: string; activity: ProviderActivityPart }
  | { kind: "activity-group"; key: string; activities: ProviderActivityPart[] }
  | { kind: "subagent-tool"; key: string; tool: ToolPart }
  | { kind: "subagent-activity"; key: string; node: ActivityNode };

/**
 * The turn's thinking, tools and activity in the order they happened.
 *
 * Threads saved before steps were recorded have none; they keep the old fixed
 * order of thinking, then tools, then activity. A part no step names is added
 * at the end rather than dropped, so a missed event can never hide a tool.
 */
export function orderedSteps(message: TraceMessage): TraceStep[] {
  const tools = new Map(message.tools.map((tool) => [tool.id, tool]));
  const activities = new Map(
    (message.providerActivity ?? []).map((activity) => [activity.id, activity])
  );
  const steps: TraceStep[] = [];
  const seenTools = new Set<string>();
  const seenActivities = new Set<string>();

  const recorded = message.steps ?? [
    ...(message.thinking ? [{ kind: "thinking" as const, text: message.thinking }] : []),
  ];
  recorded.forEach((step, index) => {
    if (step.kind === "thinking") {
      if (step.text.trim()) {
        steps.push({ kind: "thinking", key: `thinking-${index}`, text: step.text, live: false });
      }
      return;
    }
    if (step.kind === "tool") {
      const tool = tools.get(step.id);
      if (tool && !seenTools.has(tool.id)) {
        seenTools.add(tool.id);
        steps.push({ kind: "tool", tool });
      }
      return;
    }
    const activity = activities.get(step.id);
    if (activity && !seenActivities.has(activity.id)) {
      seenActivities.add(activity.id);
      steps.push({ kind: "activity", activity });
    }
  });
  for (const tool of message.tools) {
    if (!seenTools.has(tool.id)) steps.push({ kind: "tool", tool });
  }
  for (const activity of message.providerActivity ?? []) {
    if (!seenActivities.has(activity.id)) steps.push({ kind: "activity", activity });
  }

  // Only the newest block can still be thinking, and only until anything —
  // a tool, an activity, the reply — comes after it.
  const last = steps[steps.length - 1];
  if (
    last?.kind === "thinking" &&
    message.streaming &&
    message.text.length === (message.thinkingTextLength ?? 0)
  ) {
    steps[steps.length - 1] = { ...last, live: true };
  }
  return steps;
}

/** Work Zest handed to another agent: a CLI worker or a board feature card. */
export function isSubagentTool(tool: ToolPart): boolean {
  return (
    tool.name === "delegate_external" ||
    tool.name === "delegate_feature" ||
    tool.metadata?.kind === "delegation"
  );
}

/**
 * Claude Code names its subagent tool `Task` on `--tools` and `Agent` in the
 * events it sends, and Zest titles the row `<tool> <description>`. The title is
 * all the wire carries, so it is what identifies one.
 */
const AGENT_TITLE = /^(Agent|Task)(?:\s+(.*))?$/s;

export type AgentTitle = {
  /** What the subagent was asked to do. */
  description: string;
  /** The step it reported last, which Zest appends after `: `. */
  progress?: string;
};

export function parseAgentTitle(title: string): AgentTitle | null {
  const match = AGENT_TITLE.exec(title.trim());
  if (!match) return null;
  const rest = (match[2] ?? "").trim();
  const split = rest.indexOf(": ");
  if (split < 0) return { description: rest };
  return {
    description: rest.slice(0, split).trim(),
    progress: rest.slice(split + 2).trim() || undefined,
  };
}

/**
 * The activities that ran inside other activities, keyed by the parent.
 * A child whose parent never arrived stays a top-level line.
 */
function activityTree(activities: ProviderActivityPart[]): {
  roots: Set<string>;
  nodes: Map<string, ActivityNode>;
} {
  const nodes = new Map<string, ActivityNode>(
    activities.map((activity) => [activity.id, { activity, children: [] }])
  );
  const roots = new Set<string>();
  for (const activity of activities) {
    const parent =
      activity.parentId && activity.parentId !== activity.id
        ? nodes.get(activity.parentId)
        : undefined;
    if (parent) parent.children.push(nodes.get(activity.id) as ActivityNode);
    else roots.add(activity.id);
  }
  return { roots, nodes };
}

/**
 * Lines that never fold into an "N tool calls" group.
 *
 * An approval is a question the user has to see; an HTML document is its own
 * card; a subagent is a whole run of work, and folding it away is exactly how
 * it went unnoticed.
 */
function toolStandsAlone(tool: ToolPart): boolean {
  return (
    tool.status === "awaiting_approval" ||
    isSubagentTool(tool) ||
    Boolean(htmlDocumentFromMetadata(tool.metadata))
  );
}

function isSettled(tool: ToolPart): boolean {
  return tool.status === "done" || tool.status === "error";
}

/**
 * Turn a message into the lines of its `Trace`.
 *
 * Contiguous plain tool calls fold into one Chain of Thought line once there
 * are `threshold` of them, as do contiguous provider activities. Thinking,
 * approvals, HTML cards and subagents break a run, so the order stays honest.
 */
export function buildTrace(
  message: TraceMessage,
  threshold: number = COLLAPSE_THRESHOLD
): TraceItem[] {
  const { roots, nodes } = activityTree(message.providerActivity ?? []);
  const items: TraceItem[] = [];
  let tools: ToolPart[] = [];
  let activities: ProviderActivityPart[] = [];

  const flushTools = () => {
    if (tools.length >= threshold) {
      items.push({
        kind: "tool-group",
        key: `tools-${tools[0].id}`,
        tools,
        summary: summarizeTools(tools.filter(isSettled)),
      });
    } else {
      for (const tool of tools) items.push({ kind: "tool", key: tool.id, tool });
    }
    tools = [];
  };
  const flushActivities = () => {
    if (activities.length >= threshold) {
      items.push({
        kind: "activity-group",
        key: `activities-${activities[0].id}`,
        activities,
      });
    } else {
      for (const activity of activities) {
        items.push({ kind: "activity", key: activity.id, activity });
      }
    }
    activities = [];
  };
  const flush = () => {
    flushTools();
    flushActivities();
  };

  for (const step of orderedSteps(message)) {
    if (step.kind === "thinking") {
      flush();
      items.push(step);
      continue;
    }
    if (step.kind === "tool") {
      if (isSubagentTool(step.tool)) {
        flush();
        items.push({ kind: "subagent-tool", key: step.tool.id, tool: step.tool });
      } else if (toolStandsAlone(step.tool)) {
        flush();
        items.push({ kind: "tool", key: step.tool.id, tool: step.tool });
      } else {
        flushActivities();
        tools.push(step.tool);
      }
      continue;
    }
    const { activity } = step;
    if (!roots.has(activity.id)) continue;
    const node = nodes.get(activity.id) as ActivityNode;
    if (node.children.length > 0 || parseAgentTitle(activity.title)) {
      flush();
      items.push({ kind: "subagent-activity", key: activity.id, node });
    } else {
      flushTools();
      activities.push(activity);
    }
  }
  flush();
  return items;
}

export type StepStatus = "running" | "error" | "pending" | undefined;

/** The status a whole run shows: running beats failed beats done. */
export function runStatus(statuses: ReadonlyArray<string>): StepStatus {
  if (statuses.some((status) => status === "running")) return "running";
  if (statuses.some((status) => status === "error")) return "error";
  return undefined;
}
