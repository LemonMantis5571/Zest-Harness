import { useMemo } from "react";

import { ActivityGroup, ActivityLine } from "@/components/ActivityLine";
import { SubagentActivityLine, SubagentToolLine } from "@/components/SubagentLine";
import { ThinkingReasoning } from "@/components/ThinkingReasoning";
import { ToolLine } from "@/components/ToolCallRow";
import { ToolRunGroup } from "@/components/ToolRunGroup";
import { Trace } from "@/components/ui/trace";
import { buildTrace } from "@/lib/assistantTrace";
import type { ChatMessage, DelegationJob } from "@/lib/types";

type Props = {
  message: Extract<ChatMessage, { role: "assistant" }>;
  /** Whose loop produced provider activity. A CLI loop is not always Claude Code. */
  providerLabel: string;
  /** The board, for feature cards this turn created. */
  delegationJobs?: ReadonlyArray<DelegationJob>;
  onOpenDiff?: (path: string, diff: string) => void;
  onOpenDelegation?: (jobId: string) => void;
  className?: string;
};

/**
 * The steps of an assistant turn as one `Trace`, in the order they happened:
 * reasoning, tool calls and their Chain of Thought runs, provider activity,
 * and subagents, which always get a line of their own.
 *
 * The reply is not part of it; it renders under the trace.
 */
export function AssistantTrace({
  message,
  providerLabel,
  delegationJobs,
  onOpenDiff,
  onOpenDelegation,
  className,
}: Props) {
  const items = useMemo(() => buildTrace(message), [message]);
  if (items.length === 0) return null;

  return (
    <Trace aria-label="Steps" data-slot="assistant-trace" className={className}>
      {items.map((item) => {
        switch (item.kind) {
          case "thinking":
            return <ThinkingReasoning key={item.key} text={item.text} live={item.live} />;
          case "tool":
            return <ToolLine key={item.key} tool={item.tool} onOpenDiff={onOpenDiff} />;
          case "tool-group":
            return (
              <ToolRunGroup
                key={item.key}
                tools={item.tools}
                summary={item.summary}
                onOpenDiff={onOpenDiff}
              />
            );
          case "activity":
            return (
              <ActivityLine
                key={item.key}
                activity={item.activity}
                providerLabel={providerLabel}
              />
            );
          case "activity-group":
            return (
              <ActivityGroup
                key={item.key}
                activities={item.activities}
                providerLabel={providerLabel}
              />
            );
          case "subagent-tool": {
            const jobId =
              item.tool.metadata?.kind === "delegation" ? item.tool.metadata.job_id : undefined;
            return (
              <SubagentToolLine
                key={item.key}
                tool={item.tool}
                job={jobId ? delegationJobs?.find((job) => job.jobId === jobId) : undefined}
                onOpenDiff={onOpenDiff}
                onOpenDelegation={onOpenDelegation}
              />
            );
          }
          case "subagent-activity":
            return (
              <SubagentActivityLine
                key={item.key}
                node={item.node}
                providerLabel={providerLabel}
              />
            );
        }
      })}
    </Trace>
  );
}
