import { UserIcon } from "lucide-react";

import { cn } from "@/lib/utils";

type AvatarProps = {
  avatarDataUrl?: string;
  displayName?: string;
  className?: string;
};

/**
 * The avatar on its own, as a `span`.
 *
 * Split out from the button because the sidebar's profile row is itself a
 * button, and a button inside a button is invalid markup that browsers resolve
 * by dropping one of them.
 */
export function UserAvatar({ avatarDataUrl, displayName, className }: AvatarProps) {
  const initial = displayName?.trim()?.charAt(0)?.toUpperCase() ?? "";

  return (
    <span
      className={cn(
        "grid size-7 place-items-center overflow-hidden rounded-md bg-card ring-1 ring-border",
        className
      )}
    >
      {avatarDataUrl ? (
        <img src={avatarDataUrl} alt="" className="size-full object-cover" />
      ) : initial ? (
        <span className="text-[12px] font-semibold text-foreground">{initial}</span>
      ) : (
        <UserIcon className="size-3.5 text-muted-foreground" />
      )}
    </span>
  );
}

type Props = AvatarProps & {
  title?: string;
  onClick: () => void;
  avatarClassName?: string;
};

export function UserAvatarButton({
  avatarDataUrl,
  displayName,
  title = "User settings",
  onClick,
  className,
  avatarClassName,
}: Props) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={cn(
        "grid place-items-center cursor-pointer rounded-md outline-none transition-colors",
        "hover:[&>span]:ring-primary/50 focus-visible:ring-2 focus-visible:ring-ring/50",
        className
      )}
    >
      <UserAvatar
        avatarDataUrl={avatarDataUrl}
        displayName={displayName}
        className={avatarClassName}
      />
    </button>
  );
}
