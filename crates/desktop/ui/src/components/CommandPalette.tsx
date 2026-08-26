import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  CommandIcon,
  SearchIcon,
  XIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { getBackend } from "@/lib/backend";
import type { CommandView } from "@/lib/types";
import { cn } from "@/lib/utils";

export type PaletteAction = {
  id: string;
  label: string;
  description: string;
  shortcut?: string;
  run: () => void;
};

type Props = {
  open: boolean;
  actions: PaletteAction[];
  onClose: () => void;
  onCommand: (name: string) => void;
};

type PaletteItem =
  | { kind: "action"; item: PaletteAction }
  | { kind: "command"; item: CommandView };

export function CommandPalette({ open, actions, onClose, onCommand }: Props) {
  const [commands, setCommands] = useState<CommandView[]>([]);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setIndex(0);
    inputRef.current?.focus();
    void getBackend()
      .listCommands()
      .then(setCommands)
      .catch(() => setCommands([]));
  }, [open]);

  const items = useMemo<PaletteItem[]>(() => {
    const normalized = query.trim().toLowerCase();
    const next: PaletteItem[] = actions
      .filter((item) =>
        !normalized || `${item.label} ${item.description}`.toLowerCase().includes(normalized)
      )
      .map((item) => ({ kind: "action", item }));
    next.push(
      ...commands
        .filter((item) =>
          !normalized || `/${item.name} ${item.description}`.toLowerCase().includes(normalized)
        )
        .map((item): PaletteItem => ({ kind: "command", item }))
    );
    return next.slice(0, 24);
  }, [actions, commands, query]);

  useEffect(() => {
    setIndex((value) => Math.min(value, Math.max(0, items.length - 1)));
  }, [items.length]);

  if (!open) return null;

  function run(item: PaletteItem) {
    if (item.kind === "command") onCommand(item.item.name);
    else item.item.run();
    onClose();
  }

  return (
    <div className="absolute inset-0 z-40 flex items-start justify-center bg-black/20 px-3 pt-3" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <div role="dialog" aria-label="Command palette" className="w-full max-w-[520px] overflow-hidden rounded-xl border border-border/80 bg-popover text-popover-foreground shadow-2xl">
        <div className="flex h-10 items-center gap-2 border-b border-border/70 px-3 transition-colors focus-within:border-ring/70 focus-within:ring-2 focus-within:ring-ring/30">
          <SearchIcon className="size-4 shrink-0 text-muted-foreground" />
          <Input
            ref={inputRef}
            value={query}
            placeholder="Search commands, skills, and actions"
            aria-label="Search commands, skills, and actions"
            className="h-8 min-w-0 flex-1 rounded-none border-0 bg-transparent px-0 text-sm shadow-none focus-visible:border-0 focus-visible:ring-0"
            onChange={(event) => {
              setQuery(event.target.value);
              setIndex(0);
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                onClose();
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                setIndex((value) => (value + 1) % Math.max(1, items.length));
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setIndex((value) => (value - 1 + items.length) % Math.max(1, items.length));
              } else if (event.key === "Enter" && items[index]) {
                event.preventDefault();
                run(items[index]);
              }
            }}
          />
          {query ? (
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title="Clear search"
              aria-label="Clear search"
              className="shrink-0 text-muted-foreground"
              onClick={() => {
                setQuery("");
                setIndex(0);
                inputRef.current?.focus();
              }}
            >
              <XIcon />
            </Button>
          ) : (
            <kbd className="shrink-0 rounded border border-border/70 px-1.5 py-0.5 text-[10px] text-muted-foreground">
              Esc
            </kbd>
          )}
        </div>
        <div className="max-h-[min(480px,60vh)] overflow-y-auto p-1.5">
          {items.length === 0 ? (
            <Empty className="min-h-36 rounded-lg border border-dashed border-border/60 bg-card/40 px-4 py-8">
              <EmptyHeader className="gap-1.5">
                <EmptyMedia variant="icon">
                  <SearchIcon className="text-muted-foreground" />
                </EmptyMedia>
                <EmptyTitle>No matching actions</EmptyTitle>
                <EmptyDescription>Try a different search.</EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : (
            items.map((entry, itemIndex) => {
              const label = entry.kind === "command" ? `/${entry.item.name}` : entry.item.label;
              const description = entry.item.description;
              const shortcut = entry.kind === "action" ? entry.item.shortcut : undefined;
              return (
                <button
                  key={`${entry.kind}-${entry.kind === "command" ? entry.item.name : entry.item.id}`}
                  type="button"
                  className={cn(
                    "flex h-9 w-full items-center gap-2 rounded-md px-2 text-left transition-colors",
                    itemIndex === index
                      ? "bg-secondary text-foreground"
                      : "text-foreground/85 hover:bg-accent/70"
                  )}
                  onMouseEnter={() => setIndex(itemIndex)}
                  onClick={() => run(entry)}
                >
                  <span className="grid size-6 shrink-0 place-items-center rounded-md bg-background/60 text-muted-foreground">
                    {entry.kind === "command" ? <CommandIcon className="size-3.5" /> : <SearchIcon className="size-3.5" />}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-medium">{label}</span>
                    <span className="block truncate text-[11px] text-muted-foreground">{description}</span>
                  </span>
                  {shortcut ? <kbd className="text-[10px] text-muted-foreground">{shortcut}</kbd> : null}
                </button>
              );
            })
          )}
        </div>
        <div className="flex items-center gap-3 border-t border-border/60 px-3 py-2 text-[10px] text-muted-foreground">
          <span className="inline-flex items-center gap-1"><ArrowUpIcon className="size-3" /><ArrowDownIcon className="size-3" /> Navigate</span>
          <span>Enter Run</span>
          <Button type="button" variant="ghost" size="sm" className="ml-auto h-6 px-2 text-[10px]" onClick={onClose}>Close</Button>
        </div>
      </div>
    </div>
  );
}
