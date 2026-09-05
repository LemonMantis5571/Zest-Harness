import { useRef, useState, type KeyboardEvent } from "react";

/** Roving focus without selection: native button Enter/Space still commits. */
export function useOptionNavigation(
  keys: string[],
  selectedKey: string,
  disabled = false,
  orientation: "vertical" | "horizontal" = "vertical",
) {
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const activeKey = focusedKey && keys.includes(focusedKey)
    ? focusedKey : keys.includes(selectedKey) ? selectedKey : keys[0];

  function focus(key: string | undefined) {
    if (disabled || key === undefined) return;
    const button = buttons.current.get(key);
    if (!button || button.disabled) return;
    setFocusedKey(key);
    button.focus();
  }

  return {
    focus,
    activeKey,
    optionProps(key: string) {
      return {
        ref: (node: HTMLButtonElement | null) => {
          if (node) buttons.current.set(key, node);
          else buttons.current.delete(key);
        },
        tabIndex: !disabled && key === activeKey ? 0 : -1,
        onFocus: () => setFocusedKey(key),
        onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => {
          const index = keys.indexOf(key);
          let next: number;
          if (event.key === "ArrowDown" || (orientation === "horizontal" && event.key === "ArrowRight")) next = (index + 1) % keys.length;
          else if (event.key === "ArrowUp" || (orientation === "horizontal" && event.key === "ArrowLeft")) next = (index - 1 + keys.length) % keys.length;
          else if (event.key === "Home") next = 0;
          else if (event.key === "End") next = keys.length - 1;
          else return;
          event.preventDefault();
          focus(keys[next]);
        },
      };
    },
  };
}
