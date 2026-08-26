import { CheckIcon } from "lucide-react";

import { AuthShell } from "@/components/AuthShell";
import { Button } from "@/components/ui/button";

type Props = {
  onContinue: () => void;
  continuing: boolean;
};

export function AuthSuccess({ onContinue, continuing }: Props) {
  return (
    <AuthShell>
      <header className="mb-6">
        <div className="mb-4 flex size-10 items-center justify-center rounded-full bg-primary/15 text-primary">
          <CheckIcon className="size-5" strokeWidth={2.5} />
        </div>
        <h1 className="m-0 mb-1.5 text-[22px] font-semibold leading-tight tracking-[-0.4px]">
          Authentication successful
        </h1>
        <p className="m-0 max-w-[38ch] text-[13px] leading-relaxed text-muted-foreground">
          You’re signed in. Continue in Zest — no need to return to a terminal.
        </p>
      </header>
      <footer className="mt-6 flex justify-end gap-2">
        <Button type="button" disabled={continuing} onClick={onContinue}>
          {continuing ? "Starting…" : "Continue"}
        </Button>
      </footer>
    </AuthShell>
  );
}
