import { useEffect, useRef, useState } from "react";
import { post, type Answer, type Dialog } from "../../bridge";
import { choices } from "./choices";

/** A question or approval the agent waits on, drawn like oh-my-pi's own dialog: a framed
 * box titled in its border, the detail, and a list to move through with the keyboard.
 * Only the user choosing here answers it. */
export function DialogCard({ pane, dialog }: { pane: number; dialog: Dialog }) {
  const box = useRef<HTMLDivElement>(null);
  const [typed, setTyped] = useState("");
  const [at, setAt] = useState(0);
  const list = choices(dialog);
  const [heading = "", ...detail] = [...dialog.title.split("\n"), ...dialog.message.split("\n")].filter(
    (line) => line.trim() !== "",
  );

  // A list takes the keyboard itself; a typed answer takes it in its field.
  useEffect(() => {
    if (dialog.kind !== "Input") box.current?.focus();
  }, [dialog.kind]);
  const answer = (value: Answer) => post({ type: "answer", pane, dialog: dialog.id, answer: value });
  const choose = (index: number) => {
    const choice = list[index];
    if (choice) answer(choice.answer);
  };

  return (
    <div
      className="dialog"
      ref={box}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === "Escape") {
          event.preventDefault();
          answer("Cancel");
        } else if (dialog.kind !== "Input" && event.key === "ArrowDown") {
          event.preventDefault();
          setAt((at + 1) % list.length);
        } else if (dialog.kind !== "Input" && event.key === "ArrowUp") {
          event.preventDefault();
          setAt((at + list.length - 1) % list.length);
        } else if (dialog.kind !== "Input" && event.key === "Enter") {
          event.preventDefault();
          choose(at);
        }
      }}
    >
      <div className="dialog-title">{heading}</div>
      {detail.map((line, index) => (
        <div key={index} className="dialog-line">
          {line}
        </div>
      ))}
      {dialog.kind === "Input" ? (
        <input
          className="dialog-input"
          autoFocus
          placeholder="Type your answer"
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              answer({ Value: typed });
            }
          }}
        />
      ) : (
        <div className="dialog-list">
          {list.map((choice, index) => (
            <button
              key={choice.label}
              className={"dialog-option" + (index === at ? " on" : "")}
              onMouseEnter={() => setAt(index)}
              onClick={() => choose(index)}
            >
              <span className="pointer">{index === at ? "❯" : ""}</span>
              {choice.label}
            </button>
          ))}
        </div>
      )}
      <div className="dialog-hint">
        {dialog.kind === "Input" ? "↵ submit" : "↑/↓ choose · ↵ select"} · esc dismiss
      </div>
    </div>
  );
}
