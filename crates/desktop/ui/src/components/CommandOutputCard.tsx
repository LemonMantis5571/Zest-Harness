import { useState, type ReactNode } from "react";
import { ChevronDownIcon, LightbulbIcon } from "lucide-react";

import { MarkdownActions } from "@/components/MarkdownActions";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type Props = {
  /** Slash command that produced this answer, e.g. `plan`. */
  command: string;
  /** Raw markdown source, for copy and save. */
  text: string;
  /** Rendered body. */
  children: ReactNode;
  /** Still streaming - hide actions until there is something to act on. */
  streaming?: boolean;
  /** Optional follow-up action offered under the document. */
  action?: {
    label: string;
    hint?: string;
    onClick: () => void;
    disabled?: boolean;
  };
};

/** Frames a slash-command answer as a document rather than a chat reply. */
export function CommandOutputCard({
  command,
  text,
  children,
  streaming,
  action,
}: Props) {
  const [collapsed, setCollapsed] = useState(false);
  const title = command.charAt(0).toUpperCase() + command.slice(1);

  return (
    <div className="w-full max-w-full overflow-hidden rounded-xl border border-border/70 bg-card/50">
      <div className="flex items-center gap-2 border-b border-border/50 px-3 py-2">
        <LightbulbIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          {title}
        </span>

        {!streaming ? (
          <div className="flex shrink-0 items-center gap-0.5">
            <MarkdownActions text={text} suggestedName={command} />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={collapsed ? "Expand" : "Collapse"}
              aria-expanded={!collapsed}
              onClick={() => setCollapsed((value) => !value)}
            >
              <ChevronDownIcon
                className={cn(
                  "size-3.5 transition-transform duration-150",
                  collapsed && "-rotate-90"
                )}
              />
            </Button>
          </div>
        ) : null}
      </div>

      {collapsed ? (
        <button
          type="button"
          onClick={() => setCollapsed(false)}
          className="w-full px-3 py-2 text-left text-[11px] text-muted-foreground outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40"
        >
          {text.split("\n").length} lines - click to expand
        </button>
      ) : (
        <div className="px-3 py-2.5">{children}</div>
      )}

      {action && !streaming && !collapsed ? (
        <div className="flex items-center gap-2 border-t border-border/50 px-3 py-2">
          <Button
            type="button"
            size="sm"
            variant="secondary"
            disabled={action.disabled}
            onClick={action.onClick}
          >
            {action.label}
          </Button>
          {action.hint ? (
            <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
              {action.hint}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
