import type { Command } from "../../bridge";

/** One command as the list offers it: `/security scan` is `security scan`. */
export interface Choice {
  name: string;
  description: string;
  hint: string;
}

/** Every command and subcommand, as one list. */
export function choices(commands: readonly Command[]): Choice[] {
  return commands.flatMap((command) => [
    { name: command.name, description: command.description, hint: command.hint },
    ...command.subcommands.map((sub) => ({
      name: `${command.name} ${sub.name}`,
      description: sub.description,
      hint: sub.hint,
    })),
  ]);
}

/** What the user is typing after a slash, or null when they are not writing a command.
 * Only a slash that starts the message does: `/mo` yes, `a /mo` no. */
export function typing(text: string): string | null {
  if (!text.startsWith("/")) return null;
  const query = text.slice(1);
  // Once the line is finished, what follows is the command's argument.
  return query.includes("\n") ? null : query;
}

/** The commands `query` names, best first: the name starts with it, then the name holds
 * it, then only what it does mentions it. */
export function matching(all: readonly Choice[], query: string): Choice[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [...all];
  const starts: Choice[] = [];
  const named: Choice[] = [];
  const described: Choice[] = [];
  for (const choice of all) {
    const name = choice.name.toLowerCase();
    if (name.startsWith(needle)) starts.push(choice);
    else if (name.includes(needle)) named.push(choice);
    else if (choice.description.toLowerCase().includes(needle)) described.push(choice);
  }
  return [...starts, ...named, ...described];
}
