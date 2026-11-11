import { useState } from "react";
import { Chevron } from "../../ui/Chevron";
import type { Message, Tool } from "../../bridge";

function Output({ tool }: { tool: Tool }) {
  if (tool.output === "") {
    return <div className="muted small">{tool.state === "Running" ? "Running…" : "No output"}</div>;
  }
  const diff = tool.name === "edit";
  return (
    <pre className="output">
      {tool.output.split("\n").map((line, index) => {
        const kind = diff && line.startsWith("+") ? "add" : diff && line.startsWith("-") ? "del" : "";
        return (
          <div key={index} className={kind}>
            {line || " "}
          </div>
        );
      })}
    </pre>
  );
}

/** A tool call: what it was for and its argument, with its output behind a fold. */
export function ToolCard({ entry, tool }: { entry: Message; tool: Tool }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="tool">
      <button className="fold-head" onClick={() => setOpen(!open)}>
        <Chevron open={open} />
        <span className={"dot " + tool.state.toLowerCase()} />
        <strong>{tool.name}</strong>
        {entry.text && <span className="muted">{entry.text}</span>}
        {tool.detail && <code className="detail">{tool.detail}</code>}
      </button>
      {open && <Output tool={tool} />}
    </div>
  );
}
