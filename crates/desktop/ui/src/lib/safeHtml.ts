/**
 * Provenance marker for HTML produced by a deliberately trusted renderer.
 * This is not a sanitizer; callers must establish the renderer's safety first.
 */
declare const safeHtmlBrand: unique symbol;

export type SafeHtml = string & { readonly [safeHtmlBrand]: true };
export type SafeHtmlSource = "mermaid" | "shiki";

export function markTrustedHtml(value: string, source: SafeHtmlSource): SafeHtml {
  void source;
  return value as SafeHtml;
}
