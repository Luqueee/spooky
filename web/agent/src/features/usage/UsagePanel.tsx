import { useEffect, useRef } from "react";
import { post } from "../../bridge";
import type { Agent } from "../../bridge";
import { percent, tone } from "./usage";
import { useDismiss } from "../../ui/useDismiss";

/** What the providers report about the remaining usage, centred over the pane. */
export function UsagePanel({ pane, agent, close }: { pane: number; agent: Agent; close: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  useDismiss(box, close);
  useEffect(() => post({ type: "usage", pane }), [pane]);
  return (
    <div className="scrim">
      <div className="panel" ref={box}>
        <div className="panel-head">
          <strong>Usage</strong>
          <button className="chip" onClick={() => post({ type: "usage", pane })}>
            Refresh
          </button>
        </div>
        {agent.context !== null && <div className="muted small">Context window: {agent.context}% used</div>}
        {agent.usage === null && <div className="muted">Reading usage…</div>}
        {agent.usage?.length === 0 && <div className="muted">No usage data</div>}
        {agent.usage?.map((provider) => {
          const several = provider.windows.some((w) => w.account !== provider.windows[0]?.account);
          return (
            <section key={provider.name}>
              <h4>{provider.name}</h4>
              {provider.notes.map((note) => (
                <div key={note} className="note small">
                  {note}
                </div>
              ))}
              {provider.windows.map((window, index) => (
                <div key={index} className="allowance">
                  <div>
                    <div>{window.name}</div>
                    {several && <div className="muted tiny">{window.account}</div>}
                  </div>
                  <div className="bar">
                    <div className={"fill " + tone(window.left)} style={{ width: percent(window.left) + "%" }} />
                  </div>
                  <div className={"left " + tone(window.left)}>{percent(window.left)}% left</div>
                  <div className="muted small resets">{window.resets && "resets in " + window.resets}</div>
                </div>
              ))}
            </section>
          );
        })}
      </div>
    </div>
  );
}
