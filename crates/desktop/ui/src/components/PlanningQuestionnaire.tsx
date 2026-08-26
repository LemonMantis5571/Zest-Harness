import { useState, type FormEvent } from "react";

import { Questionnaire } from "@/components/ui/questionnaire";
import type { PlanningQuestion } from "@/lib/planningQuestion";

type Props = {
  question: PlanningQuestion;
  disabled?: boolean;
  onSubmit: (answer: string) => void | Promise<void>;
};

export function PlanningQuestionnaire({ question, disabled = false, onSubmit }: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [selectedMany, setSelectedMany] = useState<string[]>([]);
  const [text, setText] = useState("");
  const [attempted, setAttempted] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const hasChoices = question.choices.length > 0;
  const multiple = hasChoices && question.multiple === true;
  const answer = hasChoices
    ? multiple
      ? selectedMany.join("\n")
      : selected ?? ""
    : text.trim();
  const invalid = attempted && !answer;

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!answer || disabled || submitting) {
      setAttempted(true);
      return;
    }
    setSubmitting(true);
    try {
      await onSubmit(answer);
    } catch {
      setSubmitting(false);
    }
  }

  return (
    <Questionnaire.Root
      aria-label="Input needed"
      items={[
        {
          name: "answer",
          required: true,
          choices: hasChoices
            ? question.choices.map(({ value }) => ({ value }))
            : undefined,
        },
      ]}
      shortcuts={hasChoices ? "letters" : undefined}
      onSubmitCapture={() => setAttempted(true)}
      onSubmit={handleSubmit}
    >
      <div className="flex items-center justify-between gap-3">
        <span className="text-[10px] font-medium uppercase tracking-[0.12em] text-muted-foreground">
          Input needed
        </span>
        <Questionnaire.Progress />
      </div>

      <Questionnaire.Item name="answer" required invalid={invalid} multiple={multiple}>
        <Questionnaire.Title>{question.prompt}</Questionnaire.Title>

        {hasChoices ? (
          <Questionnaire.Choices aria-label="Answer choices">
            {question.choices.map((choice) => (
              <Questionnaire.Choice
                key={choice.value}
                value={choice.value}
                disabled={disabled || submitting}
                onChange={(event) => {
                  if (multiple) {
                    setSelectedMany((current) =>
                      event.target.checked
                        ? [...current, event.target.value]
                        : current.filter((value) => value !== event.target.value)
                    );
                  } else {
                    setSelected(event.target.value);
                  }
                }}
              >
                {choice.label}
              </Questionnaire.Choice>
            ))}
          </Questionnaire.Choices>
        ) : (
          <Questionnaire.Input
            value={text}
            disabled={disabled || submitting}
            aria-label="Your answer"
            placeholder={question.placeholder ?? "Type your answer"}
            onChange={(event) => setText(event.target.value)}
          />
        )}

        <Questionnaire.Error>
          {hasChoices ? "Choose an option to continue." : "Enter an answer to continue."}
        </Questionnaire.Error>
      </Questionnaire.Item>

      <Questionnaire.Actions>
        <Questionnaire.Submit disabled={disabled || submitting}>
          {submitting ? "Sending…" : "Send answer"}
        </Questionnaire.Submit>
      </Questionnaire.Actions>
    </Questionnaire.Root>
  );
}
