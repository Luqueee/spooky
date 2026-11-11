import { type RefObject, useEffect } from "react";

/** Calls `close` on a press outside `ref` and on Escape. */
export function useDismiss(ref: RefObject<HTMLElement | null>, close: () => void): void {
  useEffect(() => {
    const press = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) close();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    document.addEventListener("mousedown", press);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("mousedown", press);
      document.removeEventListener("keydown", key);
    };
  }, [ref, close]);
}
