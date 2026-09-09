// node scripts/acp-timing-report.mjs path/to/timings.jsonl
import { readFileSync } from 'node:fs';

const file = process.argv[2];
if (file === '--help') {
  console.log('Usage: node scripts/acp-timing-report.mjs <timings.jsonl>');
  process.exit(0);
}
if (!file) throw new Error('Usage: node scripts/acp-timing-report.mjs <timings.jsonl>');
const rows = readFileSync(file, 'utf8').split(/\r?\n/).filter(Boolean).map(JSON.parse);
const complete = rows.filter(row => row.schema === 1 && row.completed);
const requestedLabel = row => {
  const model = row.model ?? 'unknown';
  return row.effort ? model + ' [effort=' + row.effort + ']' : model;
};
const groups = Map.groupBy(
  complete,
  row => requestedLabel(row) + '; served=' + (row.served_model ?? 'unknown') + '; ' +
    (Number.isFinite(row.marks_ms.session_reused) ? 'reused session' :
      Number.isFinite(row.marks_ms.process_reused) ? 'reused process, fresh session' : 'fresh process'),
);
const difference = (marks, end, start) =>
  Number.isFinite(marks[end]) && Number.isFinite(marks[start]) ? marks[end] - marks[start] : null;
const metrics = {
  'Setup before prompt': m => m['session/prompt:sent'],
  'Initialize RPC': m => difference(m, 'initialize:received', 'initialize:sent'),
  'Create session RPC': m => difference(m, 'session/new:received', 'session/new:sent'),
  'First text from provider start': m => m.first_text,
  'First text after prompt': m => difference(m, 'first_text', 'session/prompt:sent'),
  'Completed from provider start': m => m.completed,
};
console.log(
  rows.length +
    ' records; ' +
    complete.length +
    ' completed; ' +
    (rows.length - complete.length) +
    ' incomplete/unsupported; ' +
    complete.filter(row => row.served_model).length +
    ' with served-model metadata.',
);
for (const [configuration, runs] of groups) {
  console.log('\n' + configuration + ' (' + runs.length + ' completed runs)');
  console.table(Object.entries(metrics).map(([metric, get]) => {
    const values = runs.map(row => get(row.marks_ms)).filter(Number.isFinite).sort((a, b) => a - b);
    const percentile = p => values.length ? Number(values[Math.ceil(values.length * p) - 1].toFixed(3)) : null;
    return { metric, samples: values.length, p50_ms: percentile(0.5), p95_ms: percentile(0.95) };
  }));
}
