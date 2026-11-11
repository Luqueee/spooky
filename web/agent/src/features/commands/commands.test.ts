import { describe, expect, it } from "vitest";
import { choices, matching, typing } from "./commands";

const commands = [
  { name: "model", description: "Show current model selection", hint: "", subcommands: [] },
  {
    name: "security",
    description: "Run security scans",
    hint: "<plan|scan>",
    subcommands: [
      { name: "scan", description: "Start a scan", hint: "", subcommands: [] },
      { name: "plan", description: "Create a plan", hint: "", subcommands: [] },
    ],
  },
];

describe("slash commands", () => {
  it("offer each subcommand under its own full name", () => {
    expect(choices(commands).map((c) => c.name)).toEqual([
      "model",
      "security",
      "security scan",
      "security plan",
    ]);
  });

  it("are only being typed when the slash starts the message", () => {
    expect(typing("/mo")).toBe("mo");
    expect(typing("/")).toBe("");
    expect([typing("what /mo"), typing(""), typing("hello")]).toEqual([null, null, null]);
    expect(typing("/security scan\nnow")).toBe(null);
  });

  it("put what starts with the query before what merely holds it", () => {
    const all = choices(commands);
    expect(matching(all, "scan").map((c) => c.name)).toEqual(["security scan", "security"]);
    expect(matching(all, "SEC").map((c) => c.name)).toEqual([
      "security",
      "security scan",
      "security plan",
    ]);
    expect(matching(all, "zzz")).toEqual([]);
    expect(matching(all, " ").length).toBe(4);
  });
});
