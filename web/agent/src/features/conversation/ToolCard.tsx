import { useState } from "react";
import { Chevron } from "../../ui/Chevron";
import type { Message, Tool } from "../../bridge";
import { parseDiff, stats } from "./diff";

function Diff({ diff }: { diff: string }) {
  return (
    <div className="diff">
      {parseDiff(diff).map((line, index) =>
        line.kind === "note" ? (
          <div key={index} className="diff-row note">
            <span className="gutter" />
            <span className="sign" />
            <span className="code-text">{line.text}</span>
          </div>
        ) : (
          <div key={index} className={"diff-row " + line.kind}>
            <span className="gutter">{line.line}</span>
            <span className="sign">{line.kind === "add" ? "+" : line.kind === "del" ? "−" : ""}</span>
            <span className="code-text">{line.text || " "}</span>
          </div>
        ),
      )}
    </div>
  );
}

function Output({ tool }: { tool: Tool }) {
  if (tool.diff) return <Diff diff={tool.diff} />;
  if (tool.output === "") {
    return <div className="muted small">{tool.state === "Running" ? "Running…" : "No output"}</div>;
  }
  return <pre className="output">{tool.output}</pre>;
}

function Changes({ diff }: { diff: string }) {
  const { added, removed } = stats(parseDiff(diff));
  return (
    <span className="changes">
      <span className="plus">+{added}</span> <span className="minus">−{removed}</span>
    </span>
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
        {tool.diff && <Changes diff={tool.diff} />}
      </button>
      {open && <Output tool={tool} />}
    </div>
  );
}
