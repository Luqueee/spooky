import { post } from "../bridge";

// The web view has no Edit menu, so the editing chords do nothing by themselves. The
// clipboard belongs to the application: this asks it to copy and to paste.

function selected(): string {
  const el = document.activeElement;
  if (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) {
    return el.value.slice(el.selectionStart ?? 0, el.selectionEnd ?? 0);
  }
  return window.getSelection()?.toString() ?? "";
}

/** Carries out an editing chord (Cmd+C, X, V, A, Z); false when the key is not one. */
export function edit(event: KeyboardEvent): boolean {
  if (!event.metaKey || event.altKey || event.ctrlKey) return false;
  switch (event.key.toLowerCase()) {
    case "c":
    case "x": {
      const text = selected();
      if (text !== "") post({ type: "copy", text });
      if (event.key.toLowerCase() === "x" && text !== "") document.execCommand("delete");
      return true;
    }
    case "v":
      post({ type: "paste" });
      return true;
    case "a":
      document.execCommand("selectAll");
      return true;
    case "z":
      document.execCommand(event.shiftKey ? "redo" : "undo");
      return true;
    default:
      return false;
  }
}

/** Puts text the application read from the clipboard where the caret is. */
export function paste(text: string): void {
  document.execCommand("insertText", false, text);
}
