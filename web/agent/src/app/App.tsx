import { useView } from "../bridge";
import { Composer } from "../features/composer/Composer";
import { Conversation } from "../features/conversation/Conversation";
import { DialogCard } from "../features/dialogs/DialogCard";
import { Footer } from "./Footer";

/** The agent view: the conversation, what the agent waits on, the composer and its footer. */
export function App() {
  const { pane, agent, attachments } = useView();
  if (pane === null || agent === null) return <div className="app" />;
  return (
    <div className="app">
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
