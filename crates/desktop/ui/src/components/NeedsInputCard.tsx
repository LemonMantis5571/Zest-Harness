import { CircleHelpIcon, ShieldAlertIcon } from "lucide-react";
import type { Ref } from "react";

import { PlanningQuestionnaire } from "@/components/PlanningQuestionnaire";
import { ToolCallRow } from "@/components/ToolCallRow";
import type { PlanningQuestion } from "@/lib/planningQuestion";
import type { ApprovalChoice, ToolPart } from "@/lib/types";

type Props = {
  cardRef?: Ref<HTMLDivElement>;
  question?: PlanningQuestion;
  approval?: ToolPart;
  pendingApprovalCount?: number;
  onResolveQuestion: (questionId: string, answer: string) => Promise<void>;
  onResolveApproval: (
    approvalId: string,
    decision: ApprovalChoice
  ) => Promise<void>;
  onOpenDiff: (path: string, diff: string) => void;
};

/**
 * One anchored decision surface for the conversation.
 *
 * Questions and approvals are different actions, but they have the same UX
 * requirement: the next step must stay visible while the transcript scrolls.
 * Keeping them in one card avoids competing sticky surfaces above the composer.
 */
export function NeedsInputCard({
  cardRef,
  question,
  approval,
  pendingApprovalCount = 1,
  onResolveQuestion,
  onResolveApproval,
  onOpenDiff,
}: Props) {
  const isQuestion = Boolean(question);

  return (
    <div
      ref={cardRef}
      role="region"
      aria-label={isQuestion ? "Input needed" : "Approval needed"}
      className="pointer-events-auto mx-auto w-full max-w-[var(--chat-max)] rounded-xl border border-border/70 bg-[color-mix(in_srgb,var(--card)_94%,transparent)] p-2.5 shadow-lg backdrop-blur-xl"
    >
      <div className="flex items-center justify-between gap-2 px-1 pb-2">
        <div className="flex min-w-0 items-center gap-1.5">
          {isQuestion ? (
            <CircleHelpIcon className="size-3.5 shrink-0 text-primary/90" aria-hidden />
          ) : (
            <ShieldAlertIcon className="size-3.5 shrink-0 text-amber-400/90" aria-hidden />
          )}
          <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
            {isQuestion ? "Needs your input" : "Needs your approval"}
          </span>
        </div>
        {!isQuestion && pendingApprovalCount > 1 ? (
          <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
            1 of {pendingApprovalCount}
          </span>
        ) : null}
      </div>

      {question ? (
        <PlanningQuestionnaire
          question={question}
          onSubmit={(answer) => {
            if (!question.questionId) return;
            return onResolveQuestion(question.questionId, answer);
          }}
        />
      ) : approval ? (
        <ToolCallRow
          key={approval.id}
          tool={approval}
          asCard
          onResolveApproval={onResolveApproval}
          onOpenDiff={onOpenDiff}
        />
      ) : null}
    </div>
  );
}
