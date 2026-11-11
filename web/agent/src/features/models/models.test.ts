import { describe, expect, it } from "vitest";
import { matching, modelId, newestFirst, providerCounts, providerOf, recency, rolesOf } from "./models";

describe("model names", () => {
  it("split at the first slash only", () => {
    expect(providerOf("amazon-bedrock/anthropic.claude-3")).toBe("amazon-bedrock");
    expect(modelId("openai/gpt-5/mini")).toBe("gpt-5/mini");
    expect([providerOf("local"), modelId("local")]).toEqual(["other", "local"]);
  });

  it("are searched by their whole name, ignoring case and outer blanks", () => {
    const models = ["openai/gpt-5", "openai/o3", "anthropic/Claude-5", "other-co/gpt-mini"];
    expect(matching(models, " GPT ")).toEqual(["openai/gpt-5", "other-co/gpt-mini"]);
    expect(matching(models, "")).toHaveLength(4);
  });

  it("are counted per provider, in name order", () => {
    expect(providerCounts(["b/x", "a/y", "b/z"])).toEqual([
      ["a", 1],
      ["b", 2],
    ]);
  });

  it("report the roles they play", () => {
    const roles = [
      { role: "default", model: "p/a", thinking: null },
      { role: "plan", model: "p/b", thinking: "high" },
      { role: "smol", model: "p/a", thinking: null },
    ];
    expect(rolesOf(roles, "p/a")).toEqual(["default", "smol"]);
    expect(rolesOf(roles, "p/z")).toEqual([]);
  });

  it("read their version and release date from the name", () => {
    expect(recency("anthropic/claude-3-5-haiku-20241022-v1:0")).toEqual({ version: [3, 5], date: 20241022 });
    expect(recency("openai/gpt-5.6-luna")).toEqual({ version: [5, 6], date: 0 });
    expect(recency("ollama/llama3.1-70b").version).toEqual([3, 1]);
    expect(recency("p/mystery").version).toEqual([]);
  });

  it("sort newest first: version, then date, then name", () => {
    const models = [
      "anthropic/claude-3-5-haiku-20241022-v1:0",
      "anthropic/claude-opus-4-6",
      "anthropic/claude-haiku-4-5",
      "anthropic/claude-3-5-sonnet-20240620-v1:0",
      "anthropic/claude-3-5-sonnet-20241022-v2:0",
      "anthropic/claude-opus-4-10",
    ];
    expect([...models].sort(newestFirst)).toEqual([
      "anthropic/claude-opus-4-10",
      "anthropic/claude-opus-4-6",
      "anthropic/claude-haiku-4-5",
      "anthropic/claude-3-5-haiku-20241022-v1:0",
      "anthropic/claude-3-5-sonnet-20241022-v2:0",
      "anthropic/claude-3-5-sonnet-20240620-v1:0",
    ]);
  });
});
