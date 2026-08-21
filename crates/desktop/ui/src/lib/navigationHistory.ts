export type NavigationDestination =
  | { kind: "chat" }
  | { kind: "profile" }
  | { kind: "usage" }
  | { kind: "settings"; focusUser: boolean };

export type NavigationHistory = {
  back: NavigationDestination[];
  current: NavigationDestination | null;
  forward: NavigationDestination[];
};

export function createNavigationHistory(): NavigationHistory {
  return { back: [], current: null, forward: [] };
}

export function sameDestination(
  left: NavigationDestination | null,
  right: NavigationDestination | null
) {
  if (!left || !right || left.kind !== right.kind) return false;
  if (left.kind !== "settings" || right.kind !== "settings") return true;
  return left.focusUser === right.focusUser;
}

/** Visit a destination like a browser: record the current view and discard forward history. */
export function pushNavigation(
  history: NavigationHistory,
  destination: NavigationDestination
): NavigationHistory {
  if (sameDestination(history.current, destination)) return history;
  return {
    back: history.current ? [...history.back, history.current] : [...history.back],
    current: destination,
    forward: [],
  };
}

/** Move one step through history. Direction -1 is Back; +1 is Forward. */
export function travelNavigation(
  history: NavigationHistory,
  direction: -1 | 1
): { history: NavigationHistory; destination: NavigationDestination } | null {
  if (!history.current) return null;

  if (direction === -1) {
    const destination = history.back.at(-1);
    if (!destination) return null;
    return {
      history: {
        back: history.back.slice(0, -1),
        current: destination,
        forward: [history.current, ...history.forward],
      },
      destination,
    };
  }

  const destination = history.forward[0];
  if (!destination) return null;
  return {
    history: {
      back: [...history.back, history.current],
      current: destination,
      forward: history.forward.slice(1),
    },
    destination,
  };
}
