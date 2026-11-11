import { useMemo, useRef, useState } from "react";
import { post } from "../../bridge";
import { matching, modelId, newestFirst, providerCounts, providerOf, rolesOf } from "./models";
import type { Agent } from "../../bridge";
import { useDismiss } from "../../ui/useDismiss";

type Tab = { kind: "all" } | { kind: "roles" } | { kind: "provider"; name: string };

/** The model list: a search box, the roles and providers on the left, models on the right. */
export function ModelMenu({ pane, agent, close }: { pane: number; agent: Agent; close: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState<Tab>({ kind: "all" });
  const [active, setActive] = useState(0);
  useDismiss(box, close);

  // Searching looks through every model, not only the roles.
  const found = useMemo(() => matching(agent.models, search), [agent.models, search]);
  const counts = useMemo(() => providerCounts(found), [found]);
  const shown = useMemo(() => {
    const list = tab.kind === "provider" ? found.filter((m) => providerOf(m) === tab.name) : found;
    // Providers by name; within one, the newest models first.
    return [...list].sort((a, b) => providerOf(a).localeCompare(providerOf(b)) || newestFirst(a, b));
  }, [found, tab]);

  const choose = (model: string, thinking: string | null = null) => {
    post({ type: "setModel", pane, model });
    if (thinking) post({ type: "setThinking", pane, level: thinking });
    close();
  };
  const showingRoles = tab.kind === "roles" && search.trim() === "";

  let last = "";
  return (
    <div className="popup model-menu" ref={box}>
      <input
        className="field"
        autoFocus
        placeholder="Search models"
        autoCorrect="off"
        autoCapitalize="off"
        autoComplete="off"
        spellCheck={false}
        value={search}
        onChange={(event) => {
          setSearch(event.target.value);
          setActive(0);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            setActive(Math.min(active + 1, shown.length - 1));
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            setActive(Math.max(active - 1, 0));
          } else if (event.key === "Enter" && !event.nativeEvent.isComposing && !showingRoles) {
            const model = shown[active];
            if (model) choose(model);
          }
        }}
      />
      <div className="model-body">
        <nav>
          {agent.roles.length > 0 && (
            <button
              className={"side" + (tab.kind === "roles" ? " on" : "")}
              onClick={() => {
                setSearch("");
                setTab({ kind: "roles" });
              }}
            >
              <span>Roles</span>
              <span className="muted small">{agent.roles.length}</span>
            </button>
          )}
          <button
            className={"side" + (tab.kind === "all" ? " on" : "")}
            onClick={() => setTab({ kind: "all" })}
          >
            <span>All</span>
            <span className="muted small">{found.length}</span>
          </button>
          {counts.map(([name, count]) => (
            <button
              key={name}
              className={"side" + (tab.kind === "provider" && tab.name === name ? " on" : "")}
              onClick={() => setTab({ kind: "provider", name })}
            >
              <span>{name}</span>
              <span className="muted small">{count}</span>
            </button>
          ))}
        </nav>
        <div className="models">
          {showingRoles
            ? agent.roles.map((role) => {
                const offered = agent.models.length === 0 || agent.models.includes(role.model);
                return (
                  <button
                    key={role.role}
                    className={"item" + (agent.model === role.model ? " on" : "")}
                    disabled={!offered}
                    onClick={() => choose(role.model, role.thinking)}
                  >
                    <span className="tag">{role.role}</span>
                    <span className="grow">{role.model}</span>
                    {role.thinking && <span className="muted small">{role.thinking}</span>}
                  </button>
                );
              })
            : shown.map((model, index) => {
                const heading = tab.kind !== "provider" && providerOf(model) !== last;
                last = providerOf(model);
                return (
                  <div key={model}>
                    {heading && <div className="heading">{providerOf(model)}</div>}
                    <button
                      className={"item" + (agent.model === model ? " on" : "") + (index === active ? " active" : "")}
                      onMouseEnter={() => setActive(index)}
                      onClick={() => choose(model)}
                    >
                      <span className="grow">{modelId(model)}</span>
                      {rolesOf(agent.roles, model).map((role) => (
                        <span key={role} className="tag">
                          {role}
                        </span>
                      ))}
                    </button>
                  </div>
                );
              })}
          {!showingRoles && shown.length === 0 && <div className="muted pad">No model matches</div>}
        </div>
      </div>
    </div>
  );
}
