/** The colour class of how much of an allowance is left. */
export function tone(left: number): "ok" | "low" {
  return left <= 10 ? "low" : "ok";
}

/** Clamps a figure to a whole percentage. */
export function percent(value: number): number {
  return Math.max(0, Math.min(100, Math.round(value)));
}
