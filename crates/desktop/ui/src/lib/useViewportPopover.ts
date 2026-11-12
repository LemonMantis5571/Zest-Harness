import { useLayoutEffect, type RefObject } from "react";

/** Keep non-portalled panels reachable, including in nested split composers. */
export function useViewportPopover(
  open: boolean,
  anchorRef: RefObject<HTMLElement | null>,
  panelRef: RefObject<HTMLDivElement | null>,
) {
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    const panel = panelRef.current;
    if (!open || !anchor || !panel) return;
    const place = () => {
      panel.style.translate = "none";
      panel.style.removeProperty("max-width");
      panel.style.removeProperty("max-height");
      let leftEdge = 8;
      let topEdge = 8;
      let rightEdge = window.innerWidth - 8;
      let bottomEdge = window.innerHeight - 8;
      for (let parent = panel.parentElement; parent; parent = parent.parentElement) {
        const style = getComputedStyle(parent);
        const rect = parent.getBoundingClientRect();
        if (style.overflowX !== "visible") {
          leftEdge = Math.max(leftEdge, rect.left + parent.clientLeft + 8);
          rightEdge = Math.min(rightEdge, rect.left + parent.clientLeft + parent.clientWidth - 8);
        }
        if (style.overflowY !== "visible") {
          topEdge = Math.max(topEdge, rect.top + parent.clientTop + 8);
          bottomEdge = Math.min(bottomEdge, rect.top + parent.clientTop + parent.clientHeight - 8);
        }
      }
      const natural = panel.getBoundingClientRect();
      panel.style.maxWidth = `${Math.min(natural.width, Math.max(0, rightEdge - leftEdge))}px`;
      panel.style.maxHeight = `${Math.min(natural.height, Math.max(0, bottomEdge - topEdge))}px`;
      const bounds = panel.getBoundingClientRect();
      const maxLeft = Math.max(leftEdge, rightEdge - bounds.width);
      const maxTop = Math.max(topEdge, bottomEdge - bounds.height);
      const left = Math.max(leftEdge, Math.min(bounds.left, maxLeft));
      const top = Math.max(topEdge, Math.min(bounds.top, maxTop));
      panel.style.translate = `${left - bounds.left}px ${top - bounds.top}px`;
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(panel);
    for (let element: HTMLElement | null = anchor; element; element = element.parentElement) observer.observe(element);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, { capture: true, passive: true });
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      panel.style.removeProperty("translate");
      panel.style.removeProperty("max-width");
      panel.style.removeProperty("max-height");
    };
  }, [open, anchorRef, panelRef]);
}
