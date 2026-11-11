import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { post } from "../../bridge";
import type { Agent } from "../../bridge";

const drafts = new Map<number, string>();
const MAX_HEIGHT = 220;

/** The multiline composer: Enter sends, Shift+Enter starts a new line. */
export function Composer({ pane, agent }: { pane: number; agent: Agent }) {
  const [text, setText] = useState(drafts.get(pane) ?? "");
  const box = useRef<HTMLTextAreaElement>(null);
  const live = agent.ended === null;

  // A pane keeps its own draft.
  useEffect(() => setText(drafts.get(pane) ?? ""), [pane]);
  useEffect(() => {
    window.spooky.focusComposer = () => box.current?.focus();
    box.current?.focus();
  }, [pane]);
  // A button that went away (Stop, a dialog's choice) took the focus with it; the composer
  // gets it back, unless the user put it somewhere else.
  const waiting = agent.dialogs.length;
  useEffect(() => {
    if (waiting === 0 && !agent.busy && (!document.activeElement || document.activeElement === document.body)) {
      box.current?.focus();
    }
  }, [waiting, agent.busy]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, MAX_HEIGHT) + "px";
  }, [text]);

  const change = (value: string) => {
    drafts.set(pane, value);
    setText(value);
  };
  const ready = live && text.trim() !== "";
  const send = () => {
    if (!ready) return;
    post({ type: "send", pane, text: text.trim() });
    change("");
  };

  return (
    <div className="composer">
      <textarea
        ref={box}
        rows={1}
        value={text}
        disabled={!live}
        placeholder={live ? "Reply" : "The agent has ended"}
        onChange={(event) => change(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            send();
          }
        }}
      />
      {agent.busy && (
        <button className="stop" onClick={() => post({ type: "abort", pane })}>
          Stop
        </button>
      )}
      {live && (
        <button className={"send" + (ready ? " ready" : "")} disabled={!ready} onClick={send} title="Send">
          ↑
        </button>
      )}
    </div>
  );
}
