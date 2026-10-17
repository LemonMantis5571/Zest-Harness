import { useState, type ReactNode } from "react";
import {
  BotIcon,
  ExternalLinkIcon,
  FileDiffIcon,
  LoaderCircleIcon,
} from "lucide-react";

import { ActivityLine } from "@/components/ActivityLine";
import { TraceHoverTitle, useNow } from "@/components/TraceHoverTitle";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import {
  parseAgentTitle,
  type ActivityNode,
  type StepStatus,
} from "@/lib/assistantTrace";
import {
  agentLabel,
  delegationReport,
  jobProgress,
  jobSteps,
  stepDuration,
  type JobStep,
} from "@/lib/subagents";
import type { DelegationJob, ToolPart } from "@/lib/types";
import { cn } from "@/lib/utils";

/**
 * The shell every subagent line shares, after the AI Elements Subagent
 * pattern: a bot icon, the run as the title, the model as a badge, the
 * duration as the description, and the run's own steps and report inside.
 *
 * Framed and titled in the foreground colour on purpose: a subagent is a
 * whole run of work, and as a plain tool row it went unnoticed.
 */
function SubagentShell({
  title,
  status,
  badge,
  duration,
  children,
}: {
  title: string;
  status: StepStatus;
  badge?: string;
  duration?: string;
  /** The expanded detail; none makes the line static. */
  children?: ReactNode;
}) {
  const [open, setOpen] = useState<boolean>();
  const running = status === "running";
  const line = (
    <TraceLine status={status}>
      <TraceIcon>
        {running ? (
          <LoaderCircleIcon className="animate-spin text-primary" />
        ) : (
          <BotIcon className={cn(status !== "error" && "text-primary")} />
        )}
      </TraceIcon>
      <TraceHoverTitle
        text={title}
        className={cn("font-medium", status !== "error" && "text-foreground")}
      />
      {badge ? (
        <Badge variant="secondary" className="max-w-40 shrink truncate font-mono text-[10.5px]">
          {badge}
        </Badge>
      ) : null}
      {duration ? <TraceDescription>{duration}</TraceDescription> : null}
    </TraceLine>
  );

  return (
    <div
      data-slot="subagent"
      data-status={status ?? "done"}
      className="rounded-lg border border-primary/20 bg-primary/[0.04] px-2.5 py-2"
    >
      {children ? (
        <TraceCollapsible open={open ?? running} onOpenChange={setOpen}>
          <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
          <TraceCollapsibleContent keepMounted className="flex flex-col gap-3">
            {children}
          </TraceCollapsibleContent>
        </TraceCollapsible>
      ) : (
        <Trace>{line}</Trace>
      )}
    </div>
  );
}

function Report({ text }: { text: string }) {
  return (
    <p className="whitespace-pre-wrap text-[13px] text-foreground/85">{text}</p>
  );
}

function JobStepLine({ step }: { step: JobStep }) {
  const line = (
    <TraceLine status={step.status}>
      <TraceHoverTitle text={step.title} />
      {step.description ? <TraceDescription>{step.description}</TraceDescription> : null}
    </TraceLine>
  );
  if (!step.detail) return line;
  return (
    <TraceCollapsible>
      <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        <CodeBlock size="sm" variant={step.status === "error" ? "destructive" : "default"}>
          <CodeBlockContent wrap className="max-h-48">
            {step.detail}
          </CodeBlockContent>
        </CodeBlock>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}

/** First attempt start to last attempt end: how long the board ran the job. */
function jobDuration(job: DelegationJob, now: number): string | undefined {
  if (job.attempts.length === 0) return undefined;
  const started = Math.min(...job.attempts.map((attempt) => Number(attempt.startedAt)));
  const open = job.attempts.some((attempt) => attempt.finishedAt == null);
  const finished = open
    ? undefined
    : Math.max(...job.attempts.map((attempt) => Number(attempt.finishedAt)));
  return stepDuration(started, finished, now);
}

/**
 * `delegate_feature` and `delegate_external` calls.
 *
 * A feature card is read from the live board, so the line follows the job
 * through the worker, the review and the checks instead of freezing at the
 * moment the card was created.
 */
export function SubagentToolLine({
  tool,
  job,
  onOpenDiff,
  onOpenDelegation,
}: {
  tool: ToolPart;
  job?: DelegationJob;
  onOpenDiff?: (path: string, diff: string) => void;
  onOpenDelegation?: (jobId: string) => void;
}) {
  const meta = tool.metadata?.kind === "delegation" ? tool.metadata : undefined;
  const agent = agentLabel(meta?.provider_id ?? tool.name);
  const jobRunning =
    job?.status === "worker_running" || job?.status === "review_running";
  const now = useNow(tool.status === "running" || jobRunning);
  const jobId = job?.jobId ?? meta?.job_id;

  if (tool.name === "delegate_feature" || jobId) {
    const openBoard = jobId && onOpenDelegation ? (
      <div>
        <Button
          type="button"
          size="xs"
          variant="outline"
          onClick={() => onOpenDelegation(jobId)}
        >
          <ExternalLinkIcon data-icon="inline-start" />
          Open in Delegation
        </Button>
      </div>
    ) : null;

    if (job) {
      const progress = jobProgress(job);
      return (
        <SubagentShell
          title={`${progress.label}: ${job.title}`}
          status={progress.status}
          badge={meta?.model ? `${agentLabel(job.agent)} · ${meta.model}` : agentLabel(job.agent)}
          duration={jobDuration(job, now)}
        >
          <p className="text-[13px]">{job.objective}</p>
          <Trace>
            {jobSteps(job, now).map((step) => (
              <JobStepLine key={step.key} step={step} />
            ))}
          </Trace>
          {job.workerSummary ? <Report text={job.workerSummary} /> : null}
          {job.error ? <p className="text-[13px] text-destructive">{job.error}</p> : null}
          {openBoard}
        </SubagentShell>
      );
    }

    // The board has not reported this job (yet): say what the call did.
    const status: StepStatus =
      tool.status === "running"
        ? "running"
        : tool.status === "error"
          ? "error"
          : tool.status === "awaiting_approval"
            ? "pending"
            : undefined;
    const title =
      tool.status === "awaiting_approval"
        ? `Waiting for approval: feature card for ${agent}`
        : tool.status === "running"
          ? `Creating a feature card for ${agent}`
          : tool.status === "error"
            ? `Failed to create a feature card for ${agent}`
            : `Created a feature card for ${agent}`;
    const report = delegationReport(tool);
    return (
      <SubagentShell title={title} status={status} badge={meta?.model}>
        {report || openBoard ? (
          <>
            {report ? <Report text={report} /> : null}
            {openBoard}
          </>
        ) : null}
      </SubagentShell>
    );
  }

  const status: StepStatus =
    tool.status === "running"
      ? "running"
      : tool.status === "error"
        ? "error"
        : tool.status === "awaiting_approval"
          ? "pending"
          : undefined;
  const title =
    tool.status === "awaiting_approval"
      ? `Waiting for approval to run ${agent}`
      : tool.status === "running"
        ? `Running ${agent} as a subagent`
        : tool.status === "error"
          ? `${agent} subagent failed`
          : `Ran ${agent} as a subagent`;
  const report = delegationReport(tool);
  const diff = meta?.diff ?? tool.diff;
  const hasDetail = Boolean(report) || Boolean(diff?.trim());
  return (
    <SubagentShell
      title={title}
      status={status}
      badge={meta?.model}
      duration={stepDuration(tool.startedAt, tool.finishedAt, now)}
    >
      {hasDetail ? (
        <>
          {report ? <Report text={report} /> : null}
          {diff?.trim() && onOpenDiff ? (
            <div>
              <Button
                type="button"
                size="xs"
                variant="outline"
                onClick={() => onOpenDiff(tool.path || `${agent} changes`, diff)}
              >
                <FileDiffIcon data-icon="inline-start" />
                Open changes
              </Button>
            </div>
          ) : null}
        </>
      ) : null}
    </SubagentShell>
  );
}

/**
 * A subagent a provider-owned loop spawned, such as Claude Code's `Agent`
 * tool, with the activities it reported under it.
 */
export function SubagentActivityLine({
  node,
  providerLabel,
}: {
  node: ActivityNode;
  /** Whose loop spawned it; the badge, since the wire carries no model. */
  providerLabel: string;
}) {
  const { activity, children } = node;
  const running = activity.status === "running";
  const now = useNow(running);
  const parsed = parseAgentTitle(activity.title);
  const description = parsed?.description || activity.title;
  const status: StepStatus = running ? "running" : activity.status === "error" ? "error" : undefined;
  const title = running
    ? `Running subagent: ${description}`
    : activity.status === "error"
      ? `Subagent failed: ${description}`
      : `Ran subagent: ${description}`;
  const progress = parsed?.progress;

  return (
    <SubagentShell
      title={title}
      status={status}
      badge={providerLabel}
      duration={stepDuration(activity.startedAt, activity.finishedAt, now)}
    >
      {children.length > 0 ? (
        <Trace>
          {children.map((child) =>
            child.children.length > 0 || parseAgentTitle(child.activity.title) ? (
              <SubagentActivityLine
                key={child.activity.id}
                node={child}
                providerLabel={providerLabel}
              />
            ) : (
              <ActivityLine key={child.activity.id} activity={child.activity} />
            )
          )}
        </Trace>
      ) : progress ? (
        // Without child steps on the wire, the last step it reported is the
        // only view into the run.
        <Trace>
          <TraceLine status={running ? "running" : undefined}>
            <TraceTitle>{running ? progress : `Last step: ${progress}`}</TraceTitle>
          </TraceLine>
        </Trace>
      ) : null}
    </SubagentShell>
  );
}
