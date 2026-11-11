import type { ModelRole } from "../../bridge";

/** The provider of a "provider/id" model name. */
export function providerOf(model: string): string {
  const at = model.indexOf("/");
  return at < 0 ? "other" : model.slice(0, at);
}

/** The model's own id, without its provider. */
export function modelId(model: string): string {
  const at = model.indexOf("/");
  return at < 0 ? model : model.slice(at + 1);
}

/** The models whose full name contains the search, ignoring case. */
export function matching(models: readonly string[], search: string): string[] {
  const needle = search.trim().toLowerCase();
  return needle === "" ? [...models] : models.filter((m) => m.toLowerCase().includes(needle));
}

/** How many of the given models each provider has, by provider name. */
export function providerCounts(models: readonly string[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const model of models) {
    const name = providerOf(model);
    counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return [...counts].sort(([a], [b]) => a.localeCompare(b));
}

/** The roles a model plays. */
export function rolesOf(roles: readonly ModelRole[], model: string): string[] {
  return roles.filter((r) => r.model === model).map((r) => r.role);
}

/** How new a model is, as far as its name says: the first run of version numbers in it
 * ("4-5", "5.6", "3.1") and a release date stamp (YYYYMMDD) when it has one. Region
 * prefixes and "-v1:0" suffixes do not count. The names are all there is to go by. */
export function recency(model: string): { version: number[]; date: number } {
  let id = modelId(model).replace(/-v\d+(?::\d+)?$/, "");
  let date = 0;
  id = id.replace(/(?<!\d)(?:19|20)\d{6}(?!\d)/, (stamp) => {
    date = Number(stamp);
    return "";
  });
  const run = /\d+(?:[.-]\d+)*(?![\dA-Za-z])/.exec(id)?.[0] ?? "";
  return { version: run === "" ? [] : run.split(/[.-]/).map(Number), date };
}

function compareVersions(a: readonly number[], b: readonly number[]): number {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const difference = (a[i] ?? 0) - (b[i] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

/** Orders models newest first (a later version, then a later date), and by name after that. */
export function newestFirst(a: string, b: string): number {
  const x = recency(a);
  const y = recency(b);
  return compareVersions(y.version, x.version) || y.date - x.date || a.localeCompare(b);
}
