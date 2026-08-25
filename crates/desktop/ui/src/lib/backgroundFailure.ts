/**
 * Records that a rejected best-effort task has an intentional non-UI policy.
 * Callers should use a visible error path when the operation affects correctness.
 */
export function ignoreExpectedFailure(error: unknown, operation: string): void {
  if (import.meta.env.DEV) {
    console.debug(`[zest] background failure: ${operation}`);
  }
  void error;
}

/** Apply an explicit fallback to a best-effort task without hiding its policy. */
export function fallbackOnFailure<T>(error: unknown, fallback: T, operation: string): T {
  ignoreExpectedFailure(error, operation);
  return fallback;
}
