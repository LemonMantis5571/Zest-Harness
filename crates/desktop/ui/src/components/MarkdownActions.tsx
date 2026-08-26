import { useState } from "react";
import { CheckIcon, CopyIcon, DownloadIcon, Loader2Icon } from "lucide-react";

import { toast } from "@/components/ui/toast";
import { getBackend } from "@/lib/backend";
import {
  commandMarkdownFilename,
  suggestedMarkdownFilename,
} from "@/lib/markdownExport";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { IconSwap } from "@/components/ui/icon-swap";

type Props = {
  /** Original assistant Markdown source, not rendered HTML. */
  text: string;
  /** Optional filename override, used by slash-command document cards. */
  suggestedName?: string;
  className?: string;
};

const backend = getBackend();

export function MarkdownActions({ text, suggestedName, className }: Props) {
  const [copied, setCopied] = useState(false);
  const [saving, setSaving] = useState(false);
  const hasText = text.trim().length > 0;
  const filename = suggestedName
    ? commandMarkdownFilename(suggestedName)
    : suggestedMarkdownFilename(text);

  if (!hasText) return null;

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      toast.add({
        type: "error",
        title: "Could not copy Markdown",
        description: "Clipboard access was denied by the system.",
      });
    }
  }

  async function save() {
    if (saving) return;
    setSaving(true);
    try {
      const savedPath = await backend.saveMarkdown(filename, text);
      if (savedPath) {
        toast.add({
          type: "success",
          title: "Markdown saved",
          description: savedPath,
        });
      }
    } catch {
      toast.add({
        type: "error",
        title: "Could not save Markdown",
        description: "Zest could not save the file. Try again.",
      });
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className={cn("flex items-center gap-0.5", className)}>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        title="Copy Markdown"
        // The label carries the state too. The icon is aria-hidden, so without
        // this a screen reader gets no sign the copy happened.
        aria-label={copied ? "Copied to clipboard" : "Copy Markdown"}
        onClick={() => void copy()}
      >
        <IconSwap
          className="size-3.5"
          active={copied}
          initial={<CopyIcon className="size-3.5" />}
          swapped={<CheckIcon className="size-3.5 text-primary" />}
        />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        title={`Save as ${filename}`}
        aria-label={`Save Markdown as ${filename}`}
        disabled={saving}
        onClick={() => void save()}
      >
        <IconSwap
          className="size-3.5"
          active={saving}
          initial={<DownloadIcon className="size-3.5" />}
          swapped={<Loader2Icon className="size-3.5 animate-spin" />}
        />
      </Button>
    </div>
  );
}

