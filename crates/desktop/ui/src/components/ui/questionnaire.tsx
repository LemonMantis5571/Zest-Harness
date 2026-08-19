import { Questionnaire as QuestionnairePrimitive } from "@shadcn/react/questionnaire";
import { CheckIcon } from "lucide-react";
import type { ComponentProps } from "react";

import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

function QuestionnaireRoot({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Root>) {
  return (
    <QuestionnairePrimitive.Root
      data-slot="questionnaire"
      className={cn("flex w-full min-w-0 flex-col gap-4", className)}
      {...props}
    />
  );
}

function QuestionnaireProgress({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Progress>) {
  return (
    <QuestionnairePrimitive.Progress
      className={cn("text-[11px] font-medium text-muted-foreground", className)}
      {...props}
    />
  );
}

/**
 * `gap-4` so the question is not crowded by its own answers.
 *
 * At `gap-3` the title sat closer to the choices than the section label above
 * it sat to the title, which reads as the question belonging to the label
 * rather than to the options it introduces. Matching `Root` puts the widest
 * space where the biggest break in meaning is. `Description` and `Error` pull
 * themselves back in with `-mt-1`, because both are annotations on their
 * neighbour rather than peers of it.
 */
function QuestionnaireItem({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Item>) {
  return (
    <QuestionnairePrimitive.Item
      className={cn("flex min-w-0 flex-col gap-4 border-0 p-0 outline-none", className)}
      {...props}
    />
  );
}

function QuestionnaireTitle({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Title>) {
  return (
    <QuestionnairePrimitive.Title
      className={cn("text-sm font-medium leading-6 text-foreground", className)}
      {...props}
    />
  );
}

function QuestionnaireDescription({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Description>) {
  return (
    <QuestionnairePrimitive.Description
      className={cn("-mt-1 text-xs leading-5 text-muted-foreground", className)}
      {...props}
    />
  );
}

function QuestionnaireChoices({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Choices>) {
  return (
    <QuestionnairePrimitive.Choices
      className={cn("grid gap-2", className)}
      {...props}
    />
  );
}

function QuestionnaireChoice({ children, className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Choice>) {
  return (
    <QuestionnairePrimitive.Choice
      className={cn(
        "group/question-choice relative flex min-h-11 cursor-pointer items-center gap-3 rounded-lg border border-border/80 bg-card/30 px-3 py-2.5 text-left text-sm text-foreground transition-colors",
        "hover:border-border hover:bg-muted/50",
        "focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/25",
        "data-[checked]:border-primary/50 data-[checked]:bg-primary/10",
        "has-[>input:disabled]:pointer-events-none has-[>input:disabled]:opacity-50",
        className
      )}
      {...props}
    >
      <QuestionnairePrimitive.ChoiceInput className="sr-only" />
      <span
        aria-hidden="true"
        className="flex size-4 shrink-0 items-center justify-center rounded border border-input bg-background text-primary-foreground transition-colors group-data-[checked]/question-choice:border-primary group-data-[checked]/question-choice:bg-primary"
      >
        <CheckIcon className="size-3 opacity-0 transition-opacity group-data-[checked]/question-choice:opacity-100" />
      </span>
      <QuestionnairePrimitive.ChoiceLabel className="min-w-0 flex-1 leading-5">
        {children}
      </QuestionnairePrimitive.ChoiceLabel>
      <QuestionnairePrimitive.ChoiceShortcut className="shrink-0 text-[10px] font-medium uppercase tracking-wide text-muted-foreground" />
    </QuestionnairePrimitive.Choice>
  );
}

function QuestionnaireInput({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Input>) {
  return (
    <QuestionnairePrimitive.Input
      className={cn(
        "min-h-11 w-full rounded-lg border border-input bg-background/50 px-3 py-2.5 text-sm text-foreground outline-none transition-colors placeholder:text-muted-foreground",
        "focus:border-ring focus:ring-3 focus:ring-ring/25",
        "disabled:pointer-events-none disabled:opacity-50",
        className
      )}
      {...props}
    />
  );
}

function QuestionnaireError({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Error>) {
  return (
    <QuestionnairePrimitive.Error
      className={cn("-mt-1 text-xs text-destructive", className)}
      {...props}
    />
  );
}

function QuestionnaireActions({ className, ...props }: ComponentProps<"div">) {
  return (
    <div className={cn("flex items-center justify-end gap-2", className)} {...props} />
  );
}

function QuestionnairePrevious({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Previous>) {
  return (
    <QuestionnairePrimitive.Previous
      className={cn(buttonVariants({ variant: "ghost", size: "sm" }), className)}
      {...props}
    />
  );
}

function QuestionnaireSkip({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Skip>) {
  return (
    <QuestionnairePrimitive.Skip
      className={cn(buttonVariants({ variant: "ghost", size: "sm" }), className)}
      {...props}
    />
  );
}

function QuestionnaireNext({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Next>) {
  return (
    <QuestionnairePrimitive.Next
      className={cn(buttonVariants({ variant: "outline", size: "sm" }), className)}
      {...props}
    />
  );
}

function QuestionnaireSubmit({ className, ...props }: ComponentProps<typeof QuestionnairePrimitive.Submit>) {
  return (
    <QuestionnairePrimitive.Submit
      className={cn(buttonVariants({ variant: "default", size: "sm" }), className)}
      {...props}
    />
  );
}

export const Questionnaire = {
  Root: QuestionnaireRoot,
  Progress: QuestionnaireProgress,
  Item: QuestionnaireItem,
  Title: QuestionnaireTitle,
  Description: QuestionnaireDescription,
  Choices: QuestionnaireChoices,
  Choice: QuestionnaireChoice,
  ChoiceInput: QuestionnairePrimitive.ChoiceInput,
  ChoiceLabel: QuestionnairePrimitive.ChoiceLabel,
  ChoiceShortcut: QuestionnairePrimitive.ChoiceShortcut,
  Input: QuestionnaireInput,
  Error: QuestionnaireError,
  Actions: QuestionnaireActions,
  Previous: QuestionnairePrevious,
  Skip: QuestionnaireSkip,
  Next: QuestionnaireNext,
  Submit: QuestionnaireSubmit,
};
