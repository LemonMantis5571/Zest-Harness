import { CircleAlertIcon } from "lucide-react";

import { AuthShell } from "@/components/AuthShell";
import { BrandMark } from "@/components/BrandMark";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";

type Props = {
  title: string;
  body: string;
  hint: string;
  error: string | null;
  onCancel: () => void;
};

export function WaitingScreen({ title, body, hint, error, onCancel }: Props) {
  return (
    <AuthShell>
      <header className="mb-6">
        <div className="mb-4">
          <BrandMark />
        </div>
        <h1 className="m-0 mb-1.5 text-[22px] font-semibold leading-tight tracking-[-0.4px]">
          {title}
        </h1>
        <p className="m-0 max-w-[38ch] text-[13px] leading-relaxed text-muted-foreground">
          {body}
        </p>
      </header>

      <div className="flex items-center gap-3 rounded-lg border border-border/70 bg-card/40 px-3.5 py-3 text-[13px] text-muted-foreground">
        {error ? (
          <CircleAlertIcon className="size-3.5 shrink-0 text-amber-400" aria-hidden />
        ) : (
          <Spinner className="size-3.5 text-primary" />
        )}
        <span>{hint}</span>
      </div>

      {error ? <p className="mt-3 text-xs text-destructive">{error}</p> : null}

      <footer className="mt-6 flex justify-end gap-2">
        <Button type="button" variant="outline" onClick={onCancel}>
          {error ? "Back to providers" : "Cancel"}
        </Button>
      </footer>
    </AuthShell>
  );
}
