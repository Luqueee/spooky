import { useState } from "react";
import type { Agent } from "../bridge";
import { ModelMenu } from "../features/models/ModelMenu";
import { ThinkingMenu } from "../features/models/ThinkingMenu";
import { UsagePanel } from "../features/usage/UsagePanel";

type Open = "model" | "thinking" | "usage" | null;

/** The line under the composer: model, thinking level and state; each opens its list. */
export function Footer({ pane, agent }: { pane: number; agent: Agent }) {
  const [open, setOpen] = useState<Open>(null);
  // A list takes the focus with it when it goes; the composer gets it back.
  const close = () => {
    setOpen(null);
    requestAnimationFrame(() => window.spooky.focusComposer());
  };
  const toggle = (which: Exclude<Open, null>) => setOpen(open === which ? null : which);
  return (
    <>
      <div className="footer">
        <div className="anchor">
          <button className="quiet" onClick={() => toggle("model")}>
            {agent.model ?? "model"} <span className="muted">▾</span>
          </button>
          {open === "model" && <ModelMenu pane={pane} agent={agent} close={close} />}
        </div>
        {agent.levels.length > 0 && (
          <div className="anchor">
            <button className="quiet" onClick={() => toggle("thinking")}>
              {agent.thinking ?? "thinking"} <span className="muted">▾</span>
            </button>
            {open === "thinking" && <ThinkingMenu pane={pane} agent={agent} close={close} />}
          </div>
        )}
        <span className="grow" />
        <button className="quiet" onClick={() => toggle("usage")}>
          {agent.context !== null ? agent.context + "% · " : ""}
          {agent.backend} · {agent.ended !== null ? "Ended" : agent.busy ? "Working" : "Ready"}
        </button>
      </div>
      {open === "usage" && <UsagePanel pane={pane} agent={agent} close={close} />}
    </>
  );
}
