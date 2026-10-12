import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import {
  BotIcon,
  ChartColumnIcon,
  ChevronsUpDownIcon,
  type LucideIcon,
  ServerIcon,
  SettingsIcon,
  UserIcon,
  UserPenIcon,
} from "lucide-react";

import type { SettingsSectionId } from "@/components/SettingsPanel";
import { UserAvatar } from "@/components/UserAvatarButton";
import { formatChord, loadBindings } from "@/lib/keybindings";
import type { UserProfile } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  profile: UserProfile;
  /** Provider shown under the name, e.g. "Deepseek · No workspace". */
  providerLabel?: string;
  /** The sidebar is collapsed to its icon rail: show only the avatar. */
  compact?: boolean;
  /** The profile screen is showing. */
  profileActive?: boolean;
  onOpenProfile?: () => void;
  onOpenSettings?: (section?: SettingsSectionId) => void;
};

type Item = {
  id: string;
  label: string;
  icon: LucideIcon;
  shortcut?: string;
  run: () => void;
};

/**
 * Who you are, and everything about your account, behind one click on the
 * sidebar's profile row — the way Claude's account menu works. Settings used
 * to be a gear in the chat header, where it competed with the project actions.
 *
 * Local, like `TopbarPanel`: the Tauri webview has been fragile with portalled
 * menus, and this one only has to rise out of the sidebar footer.
 */
export function AccountMenu({
  profile,
  providerLabel,
  compact = false,
  profileActive = false,
  onOpenProfile,
  onOpenSettings,
}: Props) {
  const [open, setOpen] = useState(false);
  const [settingsShortcut, setSettingsShortcut] = useState<string>();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const name = profile.displayName.trim() || "Zest";

  useEffect(() => {
    if (!open) return;
    const closeIfOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", closeIfOutside);
    menuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    return () => document.removeEventListener("pointerdown", closeIfOutside);
  }, [open]);

  function toggle() {
    if (!open) {
      // Read on open: a rebinding in Customize should show up here.
      const chord = loadBindings()["view.settings"];
      setSettingsShortcut(chord ? formatChord(chord) : undefined);
    }
    setOpen(!open);
  }

  function close() {
    setOpen(false);
    triggerRef.current?.focus();
  }

  function choose(run: () => void) {
    setOpen(false);
    run();
  }

  const settingsAt = (section: SettingsSectionId) => () => onOpenSettings?.(section);
  const groups: Item[][] = [
    onOpenProfile
      ? [{ id: "profile", label: "Your profile", icon: UserIcon, run: onOpenProfile }]
      : [],
    onOpenSettings
      ? [
          { id: "user", label: "Name and photo", icon: UserPenIcon, run: settingsAt("user") },
          { id: "provider", label: "Provider", icon: ServerIcon, run: settingsAt("provider") },
          {
            id: "delegation",
            label: "CLI delegation",
            icon: BotIcon,
            run: settingsAt("delegation"),
          },
          { id: "usage", label: "Usage", icon: ChartColumnIcon, run: settingsAt("usage") },
        ]
      : [],
    onOpenSettings
      ? [
          {
            id: "settings",
            label: "Settings",
            icon: SettingsIcon,
            shortcut: settingsShortcut,
            run: () => onOpenSettings(),
          },
        ]
      : [],
  ].filter((group) => group.length > 0);

  // Stopped here so ChatScreen's window-level Escape (which can cancel a
  // running turn) never sees the key that closed this menu.
  function onMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (event.key === "Tab") {
      setOpen(false);
      return;
    }
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []
    );
    if (!items.length) return;
    const index = items.indexOf(document.activeElement as HTMLElement);
    const next =
      event.key === "ArrowDown"
        ? (index + 1) % items.length
        : event.key === "ArrowUp"
          ? (index - 1 + items.length) % items.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? items.length - 1
              : null;
    if (next === null) return;
    event.preventDefault();
    items[next]?.focus();
  }

  const label = `Account menu for ${name}`;

  return (
    <div ref={rootRef} className={cn("relative", compact ? "shrink-0" : "min-w-0 flex-1")}>
      <button
        ref={triggerRef}
        type="button"
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={toggle}
        onKeyDown={(event) => {
          if (event.key === "Escape" && open) {
            event.preventDefault();
            event.stopPropagation();
            close();
          } else if (event.key === "ArrowUp" && !open) {
            event.preventDefault();
            toggle();
          }
        }}
        className={cn(
          "flex cursor-pointer items-center rounded-md text-left outline-none transition-colors",
          "hover:bg-[var(--sidebar-accent)] focus-visible:ring-2 focus-visible:ring-ring/50",
          compact ? "p-0.5" : "w-full min-w-0 gap-2 px-1.5 py-1.5",
          (open || profileActive) && "bg-[var(--sidebar-accent)]"
        )}
      >
        <UserAvatar
          avatarDataUrl={profile.avatarDataUrl}
          displayName={profile.displayName}
          className="shrink-0"
        />
        {compact ? null : (
          <>
            <span className="min-w-0 flex-1 leading-tight">
              <span className="block truncate text-[13px] font-medium text-foreground">
                {name}
              </span>
              {providerLabel ? (
                <span className="block truncate text-[11px] text-muted-foreground">
                  {providerLabel}
                </span>
              ) : null}
            </span>
            <ChevronsUpDownIcon
              className="size-3.5 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
          </>
        )}
      </button>

      {open ? (
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-label="Account"
          onKeyDown={onMenuKeyDown}
          className="absolute bottom-[calc(100%+0.375rem)] left-0 z-50 w-[248px] max-w-[calc(100vw-1rem)] rounded-lg border border-border bg-popover p-1 text-popover-foreground shadow-2xl animate-in fade-in slide-in-from-bottom-1 duration-150"
        >
          <div className="px-2 pb-1.5 pt-1 leading-tight">
            <div className="truncate text-[13px] font-medium">{name}</div>
            {providerLabel ? (
              <div className="mt-0.5 truncate text-[11px] text-muted-foreground">
                {providerLabel}
              </div>
            ) : null}
          </div>
          {groups.map((group) => (
            <div
              key={group[0]?.id}
              role="group"
              className="border-t border-border/60 py-1"
            >
              {group.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  onClick={() => choose(item.run)}
                  className="flex h-8 w-full cursor-pointer items-center gap-2.5 rounded-md px-2 text-left text-[13px] outline-none transition-colors hover:bg-accent focus-visible:bg-accent"
                >
                  <item.icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  {item.shortcut ? (
                    <kbd className="shrink-0 font-sans text-[11px] text-muted-foreground">
                      {item.shortcut}
                    </kbd>
                  ) : null}
                </button>
              ))}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
