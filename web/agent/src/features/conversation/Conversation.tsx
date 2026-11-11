import { useEffect, useRef } from "react";
import type { Agent } from "../../bridge";
import { Entry } from "./Entry";

/** The scrolling conversation; it follows new text unless the user scrolled up. */
export function Conversation({ agent }: { agent: Agent }) {
  const box = useRef<HTMLDivElement>(null);
  const stuck = useRef(true);
  useEffect(() => {
    const el = box.current;
    if (el && stuck.current) el.scrollTop = el.scrollHeight;
  });
  const last = agent.messages.length - 1;
  return (
    <div
      className="conversation"
      ref={box}
      onScroll={(event) => {
        const el = event.currentTarget;
        stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      <div className="column">
        {agent.dropped > 0 && <div className="notice">{agent.dropped} earlier messages are not shown</div>}
        {agent.messages.length === 0 && <div className="empty">What can I help with?</div>}
        {agent.messages.map((entry, index) => (
          <Entry key={entry.id} entry={entry} streaming={agent.busy && index === last} />
        ))}
        {agent.ended !== null && <div className="notice">The agent ended: {agent.ended}</div>}
      </div>
    </div>
  );
}
