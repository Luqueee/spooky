import { useState } from "react";
import { post } from "../bridge";

/** Asks the app to put `text` on the clipboard, and says so for a moment. */
export function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="ghost"
      title={label}
      onClick={() => {
        post({ type: "copy", text });
        setDone(true);
        setTimeout(() => setDone(false), 1500);
      }}
    >
      {done ? "Copied" : label}
    </button>
  );
}
