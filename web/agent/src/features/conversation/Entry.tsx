import type { Message } from "../../bridge";
import { CopyButton } from "../../ui/CopyButton";
import { Markdown } from "../markdown/Markdown";
import { Thinking } from "./Thinking";
import { ToolCard } from "./ToolCard";

/** One entry of the conversation, by who wrote it. */
export function Entry({ entry, streaming }: { entry: Message; streaming: boolean }) {
  switch (entry.role) {
    case "User":
      return <div className="me">{entry.text}</div>;
    case "Assistant":
      return (
        <div className="answer">
          <Markdown text={entry.text} />
          {!streaming && <CopyButton text={entry.text} label="Copy answer" />}
        </div>
      );
    case "Thinking":
      return <Thinking entry={entry} streaming={streaming} />;
    case "Tool":
      return entry.tool ? (
        <ToolCard entry={entry} tool={entry.tool} />
      ) : (
        <div className="muted">{entry.text}</div>
      );
    default:
      return <div className="notice">{entry.text}</div>;
  }
}
