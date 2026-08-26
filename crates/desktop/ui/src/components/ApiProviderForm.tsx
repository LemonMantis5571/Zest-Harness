import { useState, type ReactNode } from "react";
import { KeyRoundIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { cn } from "@/lib/utils";

type Preset = "anthropic" | "deepseek" | "openai" | "custom";

const PRESETS: Record<
  Preset,
  { label: string; id: string; baseUrl: string; model: string; models: string[] }
> = {
  anthropic: {
    label: "Anthropic",
    id: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-opus-5",
    models: [],
  },
  deepseek: {
    label: "DeepSeek",
    id: "deepseek",
    baseUrl: "https://api.deepseek.com",
    model: "deepseek-v4-flash",
    models: ["deepseek-v4-flash", "deepseek-v4-pro"],
  },
  openai: {
    label: "OpenAI",
    id: "openai",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-5",
    models: [],
  },
  custom: { label: "Custom", id: "custom", baseUrl: "", model: "", models: [] },
};

const inputClass =
  "w-full rounded-md border border-border/80 bg-background px-2.5 py-1.5 text-xs text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-2 focus-visible:ring-ring/50";

type Props = { onDone: (id: string) => Promise<void>; onCancel: () => void };

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
      {hint ? (
        <span className="mt-1 block text-[10px] leading-relaxed text-muted-foreground/80">
          {hint}
        </span>
      ) : null}
    </label>
  );
}

export function ApiProviderForm({ onDone, onCancel }: Props) {
  const [preset, setPreset] = useState<Preset>("anthropic");
  const [id, setId] = useState(PRESETS.anthropic.id);
  const [baseUrl, setBaseUrl] = useState(PRESETS.anthropic.baseUrl);
  const [model, setModel] = useState(PRESETS.anthropic.model);
  const [models, setModels] = useState(PRESETS.anthropic.models.join(", "));
  const [credential, setCredential] = useState(PRESETS.anthropic.id);
  const [key, setKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectPreset = (next: Preset) => {
    const value = PRESETS[next];
    setPreset(next);
    setId(value.id);
    setBaseUrl(value.baseUrl);
    setModel(value.model);
    setModels(value.models.join(", "));
    setCredential(value.id);
    setError(null);
  };

  const canSave =
    key.trim().length > 0 &&
    id.trim().length > 0 &&
    model.trim().length > 0 &&
    (preset === "anthropic" || baseUrl.trim().length > 0);

  return (
    <form
      className="border-t border-border/60 bg-secondary/15 px-4 py-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (!canSave) return;
        setSaving(true);
        setError(null);
        const backend = getBackend();
        const save =
          preset === "anthropic"
            ? backend.configureAnthropicProvider({
                id: id.trim(),
                model: model.trim(),
                credential: credential.trim(),
                key,
              })
            : backend.configureApiProvider({
                id: id.trim(),
                baseUrl,
                model,
                models: models
                  .split(",")
                  .map((value) => value.trim())
                  .filter(Boolean),
                credential,
                key,
              });
        void save
          .then(async () => {
            setKey("");
            await onDone(id.trim());
          })
          .catch(() => setError("Could not save this provider. Check the details and try again."))
          .finally(() => setSaving(false));
      }}
    >
      <div className="mb-3 flex items-center gap-2">
        <span className="grid size-7 place-items-center rounded-md bg-primary/15 text-primary">
          <KeyRoundIcon className="size-3.5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="text-sm font-semibold leading-tight">
            {preset === "custom" ? "Add API provider" : `Connect ${PRESETS[preset].label}`}
          </div>
          <div className="text-[11px] text-muted-foreground">
            Use your own API key. It stays in your operating system's secure storage.
          </div>
        </div>
      </div>

      <div
        role="tablist"
        aria-label="Provider preset"
        className="mb-3 flex gap-1 rounded-lg border border-border/60 bg-background/50 p-1"
      >
        {(Object.keys(PRESETS) as Preset[]).map((item) => (
          <button
            key={item}
            type="button"
            role="tab"
            aria-selected={preset === item}
            className={cn(
              "flex-1 rounded-md px-2 py-1.5 text-[11px] font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
              preset === item
                ? "bg-secondary text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground"
            )}
            onClick={() => selectPreset(item)}
          >
            {PRESETS[item].label}
          </button>
        ))}
      </div>

      {preset === "custom" ? (
        <div className="mb-3 space-y-2.5">
          <Field label="Provider id" hint="A short name used to identify this provider.">
            <input
              value={id}
              onChange={(e) => {
                const next = e.target.value;
                setId(next);
                setCredential(next.trim());
              }}
              placeholder="e.g. local-llm"
              className={inputClass}
              autoComplete="off"
            />
          </Field>
          <Field label="Base URL">
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://api.example.com/v1"
              className={inputClass}
              type="url"
              autoComplete="off"
            />
          </Field>
          <Field label="Default model">
            <input
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder="model-name"
              className={inputClass}
              autoComplete="off"
            />
          </Field>
          <Field
            label="Allowed models"
            hint="Optional. Comma-separated list; leave empty to allow only the default model."
          >
            <input
              value={models}
              onChange={(e) => setModels(e.target.value)}
              placeholder="model-a, model-b"
              className={inputClass}
              autoComplete="off"
            />
          </Field>
        </div>
      ) : (
        <dl className="mb-3 space-y-1.5 rounded-lg border border-border/60 bg-card/50 px-3 py-2.5 text-[11px]">
          <div className="grid grid-cols-[4.5rem_1fr] gap-x-2 gap-y-0.5">
            <dt className="text-muted-foreground">Endpoint</dt>
            <dd className="truncate font-mono text-foreground/90">{baseUrl}</dd>
            <dt className="text-muted-foreground">Default</dt>
            <dd className="truncate font-mono text-foreground/90">{model}</dd>
            {models.trim() ? (
              <>
                <dt className="text-muted-foreground">Models</dt>
                <dd className="font-mono text-foreground/90">{models}</dd>
              </>
            ) : null}
          </div>
          <p className="m-0 pt-1 text-[10px] text-muted-foreground/80">
            Switch to Custom to change the endpoint or model names.
          </p>
        </dl>
      )}

      <Field label={preset === "custom" ? "API key" : `${PRESETS[preset].label} API key`} hint="Your key is never saved in zest.toml.">
        <input
          value={key}
          onChange={(e) => setKey(e.target.value)}
          placeholder="sk-…"
          className={inputClass}
          type="password"
          autoComplete="off"
          autoFocus
        />
      </Field>

      {error ? (
        <p className="mt-2 rounded-md border border-destructive/40 bg-destructive/5 px-2.5 py-1.5 text-[11px] text-destructive">
          {error}
        </p>
      ) : null}

      <div className="mt-3 flex justify-end gap-2">
        <Button type="button" size="sm" variant="ghost" disabled={saving} onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={saving || !canSave}>
          {saving ? "Saving…" : preset === "custom" ? "Save provider" : "Save & connect"}
        </Button>
      </div>
    </form>
  );
}
