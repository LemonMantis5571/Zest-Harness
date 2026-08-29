import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { SearchIcon, XIcon, ZapIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { fallbackOnFailure } from "@/lib/backgroundFailure";
import { getBackend } from "@/lib/backend";
import {
  PALETTE_FILTERS,
  buildPaletteSections,
  flattenChats,
  formatPaletteAge,
  mergeChatHits,
  shiftFilter,
  type ChatHit,
  type PaletteActionInfo,
  type PaletteFilter,
  type PaletteItem,
} from "@/lib/commandPaletteSearch";
import type { CommandView } from "@/lib/types";
import { cn } from "@/lib/utils";

export type PaletteAction = PaletteActionInfo & {
  run: () => void;
};

function Highlight({ text, query }: { text: string; query: string }) {
  const needle = query.trim();
  if (!needle) return text;
  const at = text.toLowerCase().indexOf(needle.toLowerCase());
  if (at < 0) return text;
  return (
    <>
      {text.slice(0, at)}
      <span className="font-medium text-foreground">{text.slice(at, at + needle.length)}</span>
      {text.slice(at + needle.length)}
    </>
  );
}

type Props = {
  open: boolean;
  actions: PaletteAction[];
  onClose: () => void;
  onCommand: (name: string) => void;
  onOpenChat?: (options: { root: string | null; threadId: string }) => void;
};

export function CommandPalette({
  open,
  actions,
  onClose,
  onCommand,
  onOpenChat,
}: Props) {
  const [commands, setCommands] = useState<CommandView[]>([]);
  const [chats, setChats] = useState<ChatHit[]>([]);
  const [searchHits, setSearchHits] = useState<ChatHit[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<PaletteFilter>("all");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setIndex(0);
    setFilter("all");
    setSearchHits([]);
    inputRef.current?.focus();
    const backend = getBackend();
    void Promise.all([
      backend
        .listCommands()
        .catch((error) => fallbackOnFailure(error, [], "list palette commands")),
      backend
        .listChatProjects()
        .catch((error) => fallbackOnFailure(error, [], "list palette chats")),
    ]).then(([nextCommands, projects]) => {
      setCommands(nextCommands);
      setChats(flattenChats(projects));
    });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const needle = query.trim();
    if (!needle) {
      setSearchHits([]);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      void getBackend()
        .searchChats(needle)
        .then((hits) => {
          if (!cancelled) setSearchHits(hits);
        })
        .catch((error) => {
          if (!cancelled) {
            setSearchHits(fallbackOnFailure(error, [], "search chats"));
          }
        });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [open, query]);

  const chatsForPalette = useMemo(() => {
    const needle = query.trim();
    if (!needle) return chats;
    return mergeChatHits(chats, searchHits);
  }, [chats, query, searchHits]);

  const sections = useMemo(
    () => buildPaletteSections(filter, query, chatsForPalette, actions, commands),
    [filter, query, chatsForPalette, actions, commands]
  );
  const numbered = useMemo(
    () =>
      sections.map((section, sectionIndex) => ({
        section,
        start: sections
          .slice(0, sectionIndex)
          .reduce((sum, item) => sum + item.items.length, 0),
      })),
    [sections]
  );
  const items = useMemo(
    () => sections.flatMap((section) => section.items),
    [sections]
  );

  useEffect(() => {
    setIndex((value) => Math.min(value, Math.max(0, items.length - 1)));
  }, [items.length]);

  useEffect(() => {
    if (!open) return;
    document.getElementById(`palette-item-${index}`)?.scrollIntoView({
      block: "nearest",
    });
  }, [index, open, filter, query]);

  if (!open) return null;

  function run(item: PaletteItem) {
    if (item.kind === "command") onCommand(item.item.name);
    else if (item.kind === "chat") {
      onOpenChat?.({ root: item.item.projectPath, threadId: item.item.id });
    } else {
      const action = actions.find((entry) => entry.id === item.item.id);
      action?.run();
    }
    onClose();
  }

  function move(delta: number) {
    if (items.length === 0) return;
    setIndex((value) => (value + delta + items.length) % items.length);
  }

  function onPaletteKeyDown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Enter" && items[index]) {
      event.preventDefault();
      run(items[index]);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      setFilter((value) => shiftFilter(value, -1));
      setIndex(0);
    } else if (event.key === "ArrowRight" && event.shiftKey) {
      event.preventDefault();
      setFilter((value) => shiftFilter(value, 1));
      setIndex(0);
    }
  }

  return (
    <div
      className="absolute inset-0 z-50 flex items-start justify-center bg-black/40 px-3 pt-[12vh] backdrop-blur-[2px]"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Search"
        className="flex w-full max-w-[640px] flex-col overflow-hidden rounded-2xl border border-white/[0.08] bg-popover/95 text-popover-foreground shadow-[0_24px_80px_rgba(0,0,0,0.55)] backdrop-blur-xl animate-in fade-in-0 zoom-in-95 duration-150"
        onKeyDown={onPaletteKeyDown}
      >
        <div className="flex h-12 items-center gap-3 px-4">
          <SearchIcon className="size-4 shrink-0 text-muted-foreground/80" />
          <input
            ref={inputRef}
            value={query}
            placeholder="Search chats, commands, and actions..."
            aria-label="Search chats, commands, and actions"
            autoComplete="off"
            spellCheck={false}
            className="h-10 min-w-0 flex-1 bg-transparent text-[15px] text-foreground outline-none placeholder:text-muted-foreground/70"
            onChange={(event) => {
              setQuery(event.target.value);
              setIndex(0);
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
          ) : null}
        </div>

        <div
          role="tablist"
          aria-label="Search filters"
          className="flex items-center gap-1 border-b border-white/[0.06] px-3 pb-2"
        >
          {PALETTE_FILTERS.map((item) => {
            const active = item.id === filter;
            return (
              <button
                key={item.id}
                type="button"
                role="tab"
                aria-selected={active}
                className={cn(
                  "h-7 cursor-pointer rounded-md px-2.5 text-[13px] outline-none transition-colors",
                  "focus-visible:ring-2 focus-visible:ring-ring/50",
                  active
                    ? "bg-foreground/10 text-foreground"
                    : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground"
                )}
                onClick={() => {
                  setFilter(item.id);
                  setIndex(0);
                  inputRef.current?.focus();
                }}
              >
                {item.label}
              </button>
            );
          })}
        </div>

        <div role="listbox" aria-label="Search results" className="max-h-[min(440px,52vh)] overflow-y-auto px-2 py-2">
          {items.length === 0 ? (
            <p className="px-3 py-10 text-center text-[13px] text-muted-foreground">
              {query.trim() ? "No matching results." : "Nothing to show here yet."}
            </p>
          ) : (
            numbered.map(({ section, start }) => (
              <section key={section.id} className="mb-1 last:mb-0">
                <h2 className="px-2.5 pb-1 pt-1.5 text-[11px] font-medium text-muted-foreground">
                  {section.label}
                </h2>
                <ul className="m-0 flex list-none flex-col p-0">
                  {section.items.map((entry, sectionIndex) => {
                    const itemIndex = start + sectionIndex;
                    const selected = itemIndex === index;
                    const label =
                      entry.kind === "command"
                        ? entry.item.name
                        : entry.kind === "chat"
                          ? entry.item.title
                          : entry.item.label;
                    const rowKey =
                      entry.kind === "command"
                        ? entry.item.name
                        : entry.item.id;
                    const snippet =
                      entry.kind === "chat" ? entry.item.snippet?.trim() : undefined;
                    const searchNeedle = query.trim();
                    return (
                      <li key={`${entry.kind}-${rowKey}`}>
                        <button
                          id={`palette-item-${itemIndex}`}
                          type="button"
                          role="option"
                          aria-selected={selected}
                          className={cn(
                            "flex w-full cursor-pointer gap-2.5 rounded-md px-2.5 text-left outline-none transition-colors",
                            snippet ? "items-start py-1.5" : "h-8 items-center",
                            selected
                              ? "bg-foreground/10 text-foreground"
                              : "text-foreground/90 hover:bg-foreground/5"
                          )}
                          onMouseEnter={() => setIndex(itemIndex)}
                          onClick={() => run(entry)}
                        >
                          {entry.kind === "command" ? (
                            <ZapIcon
                              className="size-3.5 shrink-0 text-emerald-400"
                              aria-hidden
                            />
                          ) : null}
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-[13px]">
                              {searchNeedle ? (
                                <Highlight text={label} query={searchNeedle} />
                              ) : (
                                label
                              )}
                            </span>
                            {snippet ? (
                              <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
                                <Highlight text={snippet} query={searchNeedle} />
                              </span>
                            ) : null}
                          </span>
                          {entry.kind === "chat" ? (
                            <span className="flex max-w-[50%] shrink-0 items-center gap-3 pt-0.5 text-[11px] text-muted-foreground">
                              <span className="truncate">{entry.item.projectName}</span>
                              <span className="tabular-nums">
                                {formatPaletteAge(entry.item.updatedAt)}
                              </span>
                            </span>
                          ) : entry.kind === "action" && entry.item.shortcut ? (
                            <span className="shrink-0 text-[11px] text-muted-foreground">
                              {entry.item.shortcut}
                            </span>
                          ) : entry.kind === "command" && entry.item.description ? (
                            <span className="max-w-[45%] shrink-0 truncate text-right text-[11px] text-muted-foreground">
                              {entry.item.description}
                            </span>
                          ) : null}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </section>
            ))
          )}
        </div>

        <div className="flex items-center gap-4 border-t border-white/[0.06] px-4 py-2 text-[11px] text-muted-foreground/80">
          <span>↑↓ Select</span>
          <span>↵ Open</span>
          <span>← or Shift + → Change filter</span>
        </div>
      </div>
    </div>
  );
}
