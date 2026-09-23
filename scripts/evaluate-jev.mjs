import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const thresholds = [0.7, 0.8, 0.9, 0.95];

export function evaluate(samples, threshold) {
  const counts = { trueAlerts: 0, falseAlerts: 0, missedIssues: 0, clearCases: 0 };
  let latencyMs = 0;
  let costUsd = 0;
  for (const sample of samples) {
    latencyMs += Number(sample.latencyMs ?? 0);
    costUsd += Number(sample.usage?.cost ?? 0);
    for (const check of sample.checks ?? []) {
      if (typeof check.expectedConcern !== "boolean" || !["clear", "concern", "unclear"].includes(check.choice) || !Number.isFinite(check.probability)) {
        throw new Error(`Invalid labeled check in ${sample.targetId ?? "unknown target"}`);
      }
      const alert = check.choice === "concern" && check.probability >= threshold;
      if (alert && check.expectedConcern) counts.trueAlerts++;
      else if (alert) counts.falseAlerts++;
      else if (check.expectedConcern) counts.missedIssues++;
      else counts.clearCases++;
    }
  }
  return {
    threshold,
    samples: samples.length,
    ...counts,
    precision: counts.trueAlerts + counts.falseAlerts ? counts.trueAlerts / (counts.trueAlerts + counts.falseAlerts) : null,
    recall: counts.trueAlerts + counts.missedIssues ? counts.trueAlerts / (counts.trueAlerts + counts.missedIssues) : null,
    meanLatencyMs: samples.length ? latencyMs / samples.length : null,
    totalCostUsd: costUsd,
  };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  const path = process.argv[2];
  if (!path) throw new Error("Usage: node scripts/evaluate-jev.mjs labeled-reviews.json");
  const samples = JSON.parse(readFileSync(path, "utf8"));
  if (!Array.isArray(samples) || samples.length === 0) throw new Error("Expected a non-empty array of labeled reviews");
  for (const result of thresholds.map((threshold) => evaluate(samples, threshold))) {
    process.stdout.write(`${JSON.stringify(result)}\n`);
  }
}
