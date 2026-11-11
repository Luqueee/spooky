import { describe, expect, it } from "vitest";
import { percent, tone } from "./usage";

describe("allowances", () => {
  it("turn low at ten percent", () => {
    expect([tone(11), tone(10), tone(0)]).toEqual(["ok", "low", "low"]);
  });
  it("clamp to whole percentages", () => {
    expect([percent(-3), percent(41.6), percent(250)]).toEqual([0, 42, 100]);
  });
});
