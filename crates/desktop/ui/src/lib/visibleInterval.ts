/** Poll only while the view is visible; refresh once when returning to it. */
export function visibleInterval(
  target: Pick<Document, "hidden" | "addEventListener" | "removeEventListener">,
  callback: () => void,
  delay: number
): () => void {
  let interval: ReturnType<typeof setInterval> | undefined;
  let stopped = false;
  const tick = () => {
    if (!stopped && !target.hidden) callback();
  };
  const sync = () => {
    if (target.hidden) {
      clearInterval(interval);
      interval = undefined;
    } else if (interval === undefined && !stopped) {
      interval = setInterval(tick, delay);
      tick();
    }
  };
  // Opening the view already fetches data. Only a visibility return fetches immediately.
  if (!target.hidden) interval = setInterval(tick, delay);
  target.addEventListener("visibilitychange", sync);
  return () => {
    stopped = true;
    clearInterval(interval);
    target.removeEventListener("visibilitychange", sync);
  };
}
