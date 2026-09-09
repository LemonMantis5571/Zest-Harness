/** Keep activation and its dependent command together when panes share a session controller. */
export function createSerialActions() {
  let tail: Promise<unknown> = Promise.resolve();
  return function run<T>(action: () => Promise<T>): Promise<T> {
    const result = tail.then(action);
    // The original result is returned to the caller for error reporting; only the queue tail recovers.
    // oxlint-disable-next-line zest/no-unowned-background-rejection
    tail = result.catch(() => undefined);
    return result;
  };
}
