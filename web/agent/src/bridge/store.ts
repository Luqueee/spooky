import { useSyncExternalStore } from "react";
import type { View } from "./types";

let current: View = { pane: null, agent: null, platform: "other" };
const listeners = new Set<() => void>();

/** Replaces what is shown; called by the desktop app on every change. */
export function update(view: View): void {
  current = view;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useView(): View {
  return useSyncExternalStore(subscribe, () => current);
}
