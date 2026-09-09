import {
  BrainCircuitIcon,
  CircleDotIcon,
  Globe2Icon,
  MoonIcon,
  SparklesIcon,
  SunIcon,
  ZapIcon,
  type LucideIcon,
} from "lucide-react";

import { ProviderIcon } from "@/components/ProviderIcon";
import { modelMark, type ModelMark } from "@/lib/modelMarks";
import { providerMark, type ProviderMark } from "@/lib/providerMarks";
import { cn } from "@/lib/utils";

type Props = {
  modelId: string;
  /** Prefer the configured route's brand when one is known. */
  providerId?: string | null;
  className?: string;
  label?: string;
};

const BRAND_MARKS = ["codex", "claude", "gemini", "deepseek", "cursor"] as const;
type BrandMark = (typeof BRAND_MARKS)[number];

type FallbackModelMark = Exclude<ModelMark, BrandMark>;

const FALLBACK_ICONS: Record<FallbackModelMark, LucideIcon> = {
  sol: SunIcon,
  terra: Globe2Icon,
  luna: MoonIcon,
  mini: ZapIcon,
  llama: BrainCircuitIcon,
  standard: SparklesIcon,
  generic: CircleDotIcon,
};

// Provider identity comes from the path shape; color stays with the active
// Zest theme in both the model picker and the composer.
const MODEL_ICON_SURFACE = "bg-secondary/80";
const MODEL_ICON_FOREGROUND = "text-muted-foreground";

function isBrandMark(mark: ModelMark | ProviderMark): mark is BrandMark {
  return (BRAND_MARKS as readonly string[]).includes(mark);
}

function brandFor(modelId: string, providerId?: string | null): BrandMark | null {
  const configured = providerMark(providerId);
  if (isBrandMark(configured)) return configured;

  const inferred = modelMark(modelId);
  return isBrandMark(inferred) ? inferred : null;
}

/** Small model identity mark used anywhere a model name is shown. */
export function ModelIcon({ modelId, providerId, className, label }: Props) {
  const brand = brandFor(modelId, providerId);
  if (brand) {
    return (
      <span
        className={cn(
          "grid size-5 shrink-0 place-items-center rounded-md",
          MODEL_ICON_SURFACE,
          className
        )}
        role={label ? "img" : undefined}
        aria-label={label}
        aria-hidden={label ? undefined : true}
      >
        <ProviderIcon
          providerId={brand}
          className={cn("size-3.5", MODEL_ICON_FOREGROUND)}
        />
      </span>
    );
  }

  const mark = modelMark(modelId);
  const fallbackMark: FallbackModelMark = isBrandMark(mark) ? "generic" : mark;
  const Icon = FALLBACK_ICONS[fallbackMark];

  return (
    <span
      className={cn(
        "grid size-5 shrink-0 place-items-center rounded-md",
        MODEL_ICON_SURFACE,
        className
      )}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <Icon className={cn("size-3.5", MODEL_ICON_FOREGROUND)} aria-hidden="true" />
    </span>
  );
}
