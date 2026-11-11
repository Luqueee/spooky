/** One line of a diff as the agent's program reports it. */
export type DiffLine =
  | { kind: "same" | "add" | "del"; line: number; text: string }
  | { kind: "note"; text: string };

const LINE = /^([ +-]) *(\d+)\|(.*)$/;

/** Reads a diff of lines like ` 1|alpha`, `-3|gamma` and `+3|GAMMA`. Anything that is not one
 * (a cut, an elision) is kept as a note. */
export function parseDiff(diff: string): DiffLine[] {
  return diff.split("\n").map((raw): DiffLine => {
    const found = LINE.exec(raw);
    if (!found) return { kind: "note", text: raw };
    const [, sign = " ", number = "0", text = ""] = found;
    return { kind: sign === "+" ? "add" : sign === "-" ? "del" : "same", line: Number(number), text };
  });
}

/** How many lines a diff adds and removes. */
export function stats(lines: readonly DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === "add") added++;
    else if (line.kind === "del") removed++;
  }
  return { added, removed };
}
