import { useEffect, useState } from "react";

/**
 * Whether a CSS media query currently matches.
 *
 * For layout decisions a stylesheet cannot express — which component to mount,
 * rather than how to style it. A `hidden md:block` on the checkpoint rail would
 * still mount it, still run its ResizeObserver, and still reserve the gutter the
 * transcript is inset by.
 *
 * Returns `false` where `matchMedia` is unavailable, so a non-browser render
 * gets the roomy layout rather than crashing.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => {
    if (typeof window === "undefined" || !window.matchMedia) return false;
    return window.matchMedia(query).matches;
  });

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const list = window.matchMedia(query);
    // Re-read on subscribe: the query can have changed between the initial
    // render and this effect, and the listener only reports transitions.
    setMatches(list.matches);
    const onChange = (event: MediaQueryListEvent) => setMatches(event.matches);
    list.addEventListener("change", onChange);
    return () => list.removeEventListener("change", onChange);
  }, [query]);

  return matches;
}
