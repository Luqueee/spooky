import { describe, expect, it } from "vitest";
import { choices } from "./choices";

const base = { id: "d", title: "t", message: "", options: [] as string[] };

describe("dialog choices", () => {
  it("offer an approval as Approve or Deny", () => {
    expect(choices({ ...base, kind: "Confirm" }).map((c) => c.answer)).toEqual([
      { Confirm: true },
      { Confirm: false },
    ]);
  });
  it("answer a question with exactly the option chosen", () => {
    const list = choices({ ...base, kind: "Select", options: ["Red", "Blue"] });
    expect(list.map((c) => [c.label, c.answer])).toEqual([
      ["Red", { Value: "Red" }],
      ["Blue", { Value: "Blue" }],
    ]);
  });
  it("offer nothing to pick for a typed answer", () => {
    expect(choices({ ...base, kind: "Input" })).toEqual([]);
  });
});
