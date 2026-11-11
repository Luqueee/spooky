import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { post } from "../../bridge";
import type { Agent, Attachment } from "../../bridge";
import { Attachments } from "./Attachments";
import { CommandMenu } from "../commands/CommandMenu";
import { choices as allChoices, matching, typing } from "../commands/commands";

const drafts = new Map<number, string>();
const MAX_HEIGHT = 220;

/** The multiline composer: Enter sends, Shift+Enter starts a new line. */
export function Composer({
  pane,
  agent,
  files,
}: {
  pane: number;
  agent: Agent;
  files: readonly Attachment[];
}) {
  const [text, setText] = useState(drafts.get(pane) ?? "");
  const [at, setAt] = useState(0);
  // Escape puts the list away until the message changes again.
  const [hidden, setHidden] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  const live = agent.ended === null;

  // A pane keeps its own draft.
  useEffect(() => setText(drafts.get(pane) ?? ""), [pane]);
  useEffect(() => {
    window.spooky.focusComposer = () => box.current?.focus();
    box.current?.focus();
  }, [pane]);
  // A button that went away (Stop, a dialog's choice) took the focus with it; the composer
  // gets it back, unless the user put it somewhere else.
  const waiting = agent.dialogs.length;
  useEffect(() => {
    if (waiting === 0 && !agent.busy && (!document.activeElement || document.activeElement === document.body)) {
      box.current?.focus();
    }
  }, [waiting, agent.busy]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, MAX_HEIGHT) + "px";
  }, [text]);

  const change = (value: string) => {
    drafts.set(pane, value);
    setText(value);
    setAt(0);
    setHidden(false);
  };

  // What the program offers after a slash, narrowed to what is typed.
  const commands = useMemo(() => allChoices(agent.commands), [agent.commands]);
  const query = live && !hidden ? typing(text) : null;
  const offered = useMemo(
    () => (query === null ? [] : matching(commands, query).slice(0, 50)),
    [commands, query],
  );
  const complete = (name: string) => change(`/${name} `);
  const ready = live && (text.trim() !== "" || files.length > 0);
  const send = () => {
    if (!ready) return;
    post({ type: "send", pane, text: text.trim() });
    change("");
  };

  return (
    <div className="composer-box">
      <CommandMenu
        choices={offered}
        active={Math.min(at, Math.max(offered.length - 1, 0))}
        onHover={setAt}
        onChoose={(choice) => complete(choice.name)}
      />
      <Attachments pane={pane} files={files} />
      <div className="composer">
      <textarea
        ref={box}
        rows={1}
        value={text}
        disabled={!live}
        placeholder={live ? "Reply" : "The agent has ended"}
        onChange={(event) => change(event.target.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (offered.length > 0) {
            const chosen = offered[Math.min(at, offered.length - 1)];
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setAt((at + 1) % offered.length);
              return;
            }
            if (event.key === "ArrowUp") {
              event.preventDefault();
              setAt((at + offered.length - 1) % offered.length);
              return;
            }
            // A command already written whole is sent, not written again.
            const written = query !== null && query.trimEnd() === chosen?.name;
            if (event.key === "Tab" && chosen) {
              event.preventDefault();
              complete(chosen.name);
              return;
            }
            if (event.key === "Enter" && chosen && !written) {
              event.preventDefault();
              complete(chosen.name);
              return;
            }
            if (event.key === "Escape") {
              event.preventDefault();
              setHidden(true);
              return;
            }
          }
          if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            send();
          }
        }}
      />
      {agent.busy && (
        <button className="stop" onClick={() => post({ type: "abort", pane })}>
          Stop
        </button>
      )}
      {live && (
        <button className={"send" + (ready ? " ready" : "")} disabled={!ready} onClick={send} title="Send">
          ↑
        </button>
      )}
      </div>
    </div>
  );
}
