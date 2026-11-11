import { post } from "../bridge";
import { edit } from "./editing";

const MODIFIERS = new Set(["Meta", "Control", "Shift", "Alt"]);

/** Hands the application the chords the page does not use, so its shortcuts keep working
 * while this view has the keyboard. */
export function forwardShortcuts(): void {
  document.addEventListener(
    "keydown",
    (event) => {
      if (!event.metaKey && !event.ctrlKey) return;
      if (MODIFIERS.has(event.key)) return;
      // Editing chords are the page's; every other chord with the command key is the app's.
      if (edit(event)) {
        event.preventDefault();
        return;
      }
      if (event.ctrlKey && !event.metaKey && event.key !== "Tab") return;
      event.preventDefault();
      post({
        type: "key",
        key: event.key,
        code: event.code,
        shift: event.shiftKey,
        alt: event.altKey,
        ctrl: event.ctrlKey,
        meta: event.metaKey,
      });
    },
    true,
  );
}

/** Typing with nothing to type into (after clicking a fold, say) goes to the composer, as
 * in a chat: the focus moves there before the key's character is inserted. */
export function typeIntoComposer(): void {
  document.addEventListener("keydown", (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey || event.key.length !== 1) return;
    const el = document.activeElement;
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) return;
    window.spooky.focusComposer();
  });
}

/** No page is ever navigated to, and files dropped on the view are not opened. */
export function refuseDrops(): void {
  document.addEventListener("dragover", (event) => event.preventDefault());
  document.addEventListener("drop", (event) => event.preventDefault());
}
