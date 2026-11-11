import { useState } from "react";
import { Chevron } from "../../ui/Chevron";
import type { Message } from "../../bridge";

/** The model's reasoning, folded until the user opens it. */
export function Thinking({ entry, streaming }: { entry: Message; streaming: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="fold">
      <button className="fold-head" onClick={() => setOpen(!open)}>
        <Chevron open={open} />
        <span className="muted">{streaming ? "Thinking…" : "Thought"}</span>
      </button>
      {open && <div className="fold-body muted">{entry.text}</div>}
    </div>
  );
}
