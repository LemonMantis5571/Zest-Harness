import { useState } from "react";
import {
  ActivityIcon,
  FilePenLineIcon,
  FileTextIcon,
  GlobeIcon,
  ListChecksIcon,
  SearchIcon,
  SparklesIcon,
  TerminalIcon,
  XIcon,
} from "lucide-react";

import { TraceHoverTitle } from "@/components/TraceHoverTitle";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
} from "@/components/ui/trace";
import { runStatus } from "@/lib/assistantTrace";
import type { ProviderActivityPart } from "@/lib/types";

/**
 * Provider titles open with the tool's own name (`Bash cargo test`,
 * `Read src/lib.rs`), so that word picks the icon.
 */
function ActivityToolIcon({ activity }: { activity: ProviderActivityPart }) {
  if (activity.status === "error") return <XIcon />;
  const tool = activity.title.split(/\s/, 1)[0]?.toLowerCase() ?? "";
  switch (tool) {
    case "bash":
    case "powershell":
    case "shell":
      return <TerminalIcon />;
    case "read":
      return <FileTextIcon />;
    case "edit":
    case "write":
    case "notebookedit":
      return <FilePenLineIcon />;
    case "grep":
    case "glob":
    case "toolsearch":
      return <SearchIcon />;
    case "webfetch":
    case "websearch":
      return <GlobeIcon />;
    case "skill":
      return <SparklesIcon />;
    default:
      return <ActivityIcon />;
  }
}

/** One step a provider-owned loop reported, as a `TraceLine`. */
export function ActivityLine({
  activity,
  providerLabel,
}: {
  activity: ProviderActivityPart;
  /** Names whose loop it was, for a line not already under a group that does. */
  providerLabel?: string;
}) {
  return (
    <TraceLine
      status={
        activity.status === "running"
          ? "running"
          : activity.status === "error"
            ? "error"
            : undefined
      }
    >
      <TraceIcon>
        <ActivityToolIcon activity={activity} />
      </TraceIcon>
      <TraceHoverTitle text={activity.title} />
      {providerLabel ? <TraceDescription>{providerLabel}</TraceDescription> : null}
    </TraceLine>
  );
}

/**
 * A run of provider activity as one Chain of Thought line, labelled with
 * whose loop ran it. Opens while a step runs, folds when they settle.
 */
export function ActivityGroup({
  activities,
  providerLabel,
}: {
  activities: ProviderActivityPart[];
  providerLabel: string;
}) {
  const [open, setOpen] = useState<boolean>();
  const status = runStatus(activities.map((activity) => activity.status));
  const running = activities.find((activity) => activity.status === "running");
  const failed = activities.filter((activity) => activity.status === "error").length;
  const count = activities.length;
  const title = running
    ? running.title
    : `Ran ${count} step${count === 1 ? "" : "s"}${failed > 0 ? `, ${failed} failed` : ""}`;

  return (
    <TraceCollapsible open={open ?? Boolean(running)} onOpenChange={setOpen}>
      <TraceCollapsibleTrigger>
        <TraceLine status={status}>
          <TraceIcon>
            <ListChecksIcon />
          </TraceIcon>
          <TraceHoverTitle text={title} />
          <TraceDescription>{providerLabel}</TraceDescription>
        </TraceLine>
      </TraceCollapsibleTrigger>
      <TraceCollapsibleContent keepMounted>
        <Trace>
          {activities.map((activity) => (
            <ActivityLine key={activity.id} activity={activity} />
          ))}
        </Trace>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}
