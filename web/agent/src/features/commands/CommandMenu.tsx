import { useEffect, useRef } from "react";
import type { Choice } from "./commands";

/** The commands that match what is typed after the slash, over the composer. The list
 * is driven from the composer: it moves and chooses, this only draws. */
export function CommandMenu({
  choices,
  active,
  onHover,
  onChoose,
}: {
  choices: readonly Choice[];
  active: number;
  onHover: (index: number) => void;
  onChoose: (choice: Choice) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    box.current?.querySelector(".item.active")?.scrollIntoView({ block: "nearest" });
  }, [active]);
  if (choices.length === 0) return null;
  return (
    <div className="popup command-menu" ref={box}>
      {choices.map((choice, index) => (
        <button
          key={choice.name}
          className={"item" + (index === active ? " active" : "")}
          onMouseEnter={() => onHover(index)}
          // The composer keeps the keyboard: choosing must not take it away.
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => onChoose(choice)}
        >
          <span className="command">/{choice.name}</span>
          {choice.hint && <span className="hint">{choice.hint}</span>}
          <span className="what">{choice.description}</span>
        </button>
      ))}
    </div>
  );
}
