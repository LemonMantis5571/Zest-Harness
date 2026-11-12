import { PROVIDER_SOURCE_LABELS, type ProviderSource } from "@/lib/models";
import { cn } from "@/lib/utils";

/** "CLI" or "API": whether the vendor's CLI or Zest runs this provider. */
export function ProviderSourceTag({
  source,
  className,
}: {
  source: ProviderSource;
  className?: string;
}) {
  const { tag, title } = PROVIDER_SOURCE_LABELS[source];
  return (
    <span
      title={title}
      className={cn(
        "inline-flex shrink-0 items-center rounded-sm border border-border/70 px-1 text-[9px] font-semibold leading-[14px] tracking-[0.04em] text-muted-foreground",
        className
      )}
    >
      {tag}
    </span>
  );
}
