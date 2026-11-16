import { useSyncExternalStore } from "react";
import type { View } from "./types";

let current: View = {
  pane: null,
  agent: null,
  attachments: [],
  platform: "other",
  appearance: "spooky",
  colors: {
    background: "#22252e",
    sidebar: "#181b22",
    hover: "#242831",
    selected: "#2b2f39",
    border: "#30343e",
    text: "#cfd3dc",
    muted: "#777d8a",
    accent: "#a0adc8",
    success: "#7db99c",
    warning: "#dab87e",
    danger: "#e2858f",
  },
};
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
