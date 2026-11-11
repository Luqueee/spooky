// What the page asks of the desktop app. The app validates every message again; the
// server validates what it forwards. Nothing here acts on its own.

import type { Answer } from "./types";

export type Action =
  | { type: "ready" }
  | { type: "send"; pane: number; text: string }
  | { type: "abort"; pane: number }
  | { type: "setModel"; pane: number; model: string }
  | { type: "setThinking"; pane: number; level: string }
  | { type: "answer"; pane: number; dialog: string; answer: Answer }
  | { type: "usage"; pane: number }
  | { type: "unattach"; pane: number; named: string }
  | { type: "copy"; text: string }
  | { type: "paste" }
  | { type: "link"; url: string; open: boolean }
  | {
      type: "key";
      key: string;
      code: string;
      shift: boolean;
      alt: boolean;
      ctrl: boolean;
      meta: boolean;
    };

interface Bridge {
  postMessage(message: string): void;
}

declare global {
  interface Window {
    ipc?: Bridge;
    spooky: { update(view: unknown): void; focusComposer(): void; paste(text: string): void };
  }
}

/** Sends an action to the desktop app; in a plain browser (development) it is logged. */
export function post(action: Action): void {
  const text = JSON.stringify(action);
  if (window.ipc) window.ipc.postMessage(text);
  else console.debug("spooky action", text);
}
