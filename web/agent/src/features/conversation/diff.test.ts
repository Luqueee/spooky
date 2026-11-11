import { describe, expect, it } from "vitest";
import { parseDiff, stats } from "./diff";

describe("diffs", () => {
  const lines = parseDiff(" 1|alpha\n 2|beta\n-3|gamma\n+3|GAMMA | x\n 4|delta");

  it("are read line by line with their numbers", () => {
    expect(lines.map((l) => l.kind)).toEqual(["same", "same", "del", "add", "same"]);
    expect(lines[3]).toEqual({ kind: "add", line: 3, text: "GAMMA | x" });
  });

  it("count what they add and remove", () => {
    expect(stats(lines)).toEqual({ added: 1, removed: 1 });
  });

  it("keep what is not a line of the diff as a note", () => {
    expect(parseDiff(" 1|a\n...\n[truncated]").slice(1)).toEqual([
      { kind: "note", text: "..." },
      { kind: "note", text: "[truncated]" },
    ]);
  });

  it("accept padded line numbers", () => {
    expect(parseDiff("+ 12|x")[0]).toEqual({ kind: "add", line: 12, text: "x" });
  });
});
