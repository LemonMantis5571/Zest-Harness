export type NowPlayingOperationResult<T> =
  | {
      committed: boolean;
      status: "success";
      value: T;
    }
  | {
      committed: boolean;
      status: "error";
      error: unknown;
    }
  | {
      committed: false;
      status: "skipped";
    };

/**
 * Serializes media reads and actions while making completion order explicit.
 *
 * A new action advances the generation immediately. Reads already queued or in
 * flight may still finish, but their result is marked stale and must not update
 * React state. This keeps the backend contract promise-based without allowing
 * an old poll to undo a newer user action.
 */
export function createNowPlayingCoordinator<T>() {
  let generation = 0;
  let disposed = false;
  let tail: Promise<void> = Promise.resolve();
  let pendingRead: Promise<NowPlayingOperationResult<T>> | null = null;

  function enqueue(
    operation: () => Promise<T>,
    operationGeneration: number
  ): Promise<NowPlayingOperationResult<T>> {
    const task = tail.then(async () => {
      if (disposed) return { committed: false, status: "skipped" } as const;

      try {
        const value = await operation();
        return {
          committed: !disposed && generation === operationGeneration,
          status: "success",
          value,
        } as const;
      } catch (error) {
        return {
          committed: !disposed && generation === operationGeneration,
          status: "error",
          error,
        } as const;
      }
    });

    // Keep the queue alive after an unexpected implementation error in the
    // coordinator itself. Backend failures are converted into an outcome above.
    tail = task.then(
      () => undefined,
      () => undefined
    );
    return task;
  }

  function read(operation: () => Promise<T>) {
    if (disposed) {
      return Promise.resolve<NowPlayingOperationResult<T>>({
        committed: false,
        status: "skipped",
      });
    }
    if (pendingRead) return pendingRead;

    const operationGeneration = generation;
    const task = enqueue(operation, operationGeneration);
    const tracked = task.finally(() => {
      if (pendingRead === tracked) pendingRead = null;
    });
    pendingRead = tracked;
    return tracked;
  }

  function action(operation: () => Promise<T>) {
    if (disposed) {
      return Promise.resolve<NowPlayingOperationResult<T>>({
        committed: false,
        status: "skipped",
      });
    }

    const operationGeneration = ++generation;
    return enqueue(operation, operationGeneration);
  }

  return {
    read,
    action,
    invalidate() {
      generation += 1;
    },
    dispose() {
      disposed = true;
      generation += 1;
    },
  };
}
