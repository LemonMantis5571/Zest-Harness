import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * Whether the user has asked for less motion.
 *
 * Needed alongside the CSS media query because `scroll-behavior` does not apply
 * to the `behavior` option of `scrollIntoView` — a scripted smooth scroll keeps
 * animating however the stylesheet is written.
 */
export function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches
}

/** `scrollIntoView` options that respect the motion preference. */
export function scrollBehavior(): ScrollBehavior {
  return prefersReducedMotion() ? "auto" : "smooth"
}
