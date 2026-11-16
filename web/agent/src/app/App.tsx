import type { CSSProperties } from "react";
import { useView } from "../bridge";
import { Composer } from "../features/composer/Composer";
import { Conversation } from "../features/conversation/Conversation";
import { DialogCard } from "../features/dialogs/DialogCard";
import { Footer } from "./Footer";

/** The agent view: the conversation, what the agent waits on, the composer and its footer. */
export function App() {
  const { pane, agent, attachments, appearance, colors } = useView();
  const themeStyle = {
    "--bg": colors.background,
    "--raise": colors.sidebar,
    "--hi": colors.hover,
    "--selected": colors.selected,
    "--line": colors.border,
    "--fg": colors.text,
    "--muted": colors.muted,
    "--accent": colors.accent,
    "--ok": colors.success,
    "--warn": colors.warning,
    "--bad": colors.danger,
  } as CSSProperties;
  if (pane === null || agent === null) {
    return <div className="app" data-theme={appearance} style={themeStyle} />;
  }
  return (
    <div className="app" data-theme={appearance} style={themeStyle}>
      <Conversation agent={agent} />
      <div className="bottom">
        <div className="column">
          {agent.dialogs.map((dialog) => (
            <DialogCard key={dialog.id} pane={pane} dialog={dialog} />
          ))}
          <Composer pane={pane} agent={agent} files={attachments} />
          {/* Keyed by pane so an open list never follows the user to another agent. */}
          <Footer key={pane} pane={pane} agent={agent} />
        </div>
      </div>
    </div>
  );
}
