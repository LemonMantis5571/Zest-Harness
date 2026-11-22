// Used only by the production-mode benchmark build. App renders its ordinary chat screen.
import { createFixtureBackend } from "@/lib/fixtureBackend";

const backend = createFixtureBackend();
const originalStart = backend.startSession.bind(backend);
let run;
let session;
backend.startSession = async (...args) => {
  run = await fetch("/workload.json").then(response => response.json());
  session = await originalStart(...args);
  const messages = run.history.map((text, i) => i % 2 === 0
    ? { id: "user-" + i, role: "user", text }
    : { id: "assistant-" + i, role: "assistant", text, thinking: "", tools: [], streaming: false });
  return { ...session, messages };
};
backend.boot = async handler => {
  window.runDesktopBenchmark = () => measure(handler);
};
export const getBackend = () => backend;

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const nextFrame = () => new Promise(resolve => requestAnimationFrame(resolve));

async function measure(handler) {
  const viewport = document.querySelector('[data-slot="message-scroller-viewport"]');
  if (!viewport) throw new Error("Missing transcript viewport");
  const visibleRows = viewport.querySelectorAll('[data-slot="message-scroller-item"]').length;
  if (visibleRows !== run.history.length) throw new Error("Expected " + run.history.length + " rows, found " + visibleRows);
  if (run.mode === "idle" || run.mode === "workspace") {
    await delay(run.seconds * 1000);
    return { seconds: run.seconds, history: visibleRows, shell: run.mode === "workspace" };
  }
  const ids = { session_id: session.sessionId, thread_id: session.threadId, turn_id: "benchmark-turn", message_id: "benchmark-answer" };
  if (run.mode === "stream") handler({ ...ids, kind: "assistant_start" });
  viewport.scrollTop = viewport.scrollHeight;
  await nextFrame();
  const gaps = [];
  let frame = 0;
  let previous = performance.now();
  const start = previous;
  function record(time) {
    if (time > previous) gaps.push(time - previous);
    previous = time;
    frame = requestAnimationFrame(record);
  }
  frame = requestAnimationFrame(record);
  let sent = 0;
  let steps = 0;
  let travel = 0;
  let direction = -1;
  while (performance.now() - start < run.seconds * 1000) {
    if (run.mode === "stream") {
      const due = Math.min(Math.floor((performance.now() - start) * run.rate / 1000), run.seconds * run.rate);
      while (sent < due) handler({ ...ids, kind: "text_delta", text: run.tokens[sent++] });
    } else if (run.mode === "scroll") {
      if (++steps % 120 === 0) direction *= -1;
      const before = viewport.scrollTop;
      viewport.scrollTop += 60 * direction;
      travel += Math.abs(viewport.scrollTop - before);
    }
    await delay(run.mode === "stream" ? 4 : 16);
  }
  if (run.mode === "stream") {
    while (sent < run.seconds * run.rate) handler({ ...ids, kind: "text_delta", text: run.tokens[sent++] });
    handler({ ...ids, kind: "done" });
  }
  await nextFrame();
  cancelAnimationFrame(frame);
  const elapsed = (performance.now() - start) / 1000;
  if (run.mode === "scroll" && !travel) throw new Error("Scroll did not move");
  const streamText = run.tokens.slice(0, sent).join("");
  if (sent && !document.getElementById("message-benchmark-answer")?.textContent.includes(streamText.trim().split(/\s+/).find(word => word.length > 6))) {
    throw new Error("Stream content did not reach transcript");
  }
  gaps.sort((a, b) => a - b);
  return { seconds: elapsed, tokens: sent, bytes: new TextEncoder().encode(streamText).length,
    frames: gaps.length, fps: gaps.length / elapsed, p95FrameMs: gaps[Math.floor(gaps.length * .95)],
    p99FrameMs: gaps[Math.floor(gaps.length * .99)], over25ms: gaps.filter(gap => gap > 25).length,
    scrollPixels: travel, viewport: [innerWidth, innerHeight], history: run.history.length };
}
