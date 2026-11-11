import { useRef } from "react";
import { post } from "../../bridge";
import type { Agent } from "../../bridge";
import { useDismiss } from "../../ui/useDismiss";

export function ThinkingMenu({ pane, agent, close }: { pane: number; agent: Agent; close: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  useDismiss(box, close);
  return (
    <div className="popup small-menu" ref={box}>
      {agent.levels.map((level) => (
        <button
          key={level}
          className={"item" + (agent.thinking === level ? " on" : "")}
          onClick={() => {
            post({ type: "setThinking", pane, level });
            close();
          }}
        >
          {level}
        </button>
      ))}
    </div>
  );
}
