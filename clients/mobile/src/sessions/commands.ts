import type { SessionCommand } from "@zwiim/sdk";

/** A `/query` being typed just before the caret, at the start or after a space. */
export interface SlashQuery {
  start: number;
  query: string;
}

export function slashQuery(text: string, caret: number): SlashQuery | null {
  const m = /(^|\s)\/(\S*)$/.exec(text.slice(0, caret));
  return m ? { start: m.index + m[1]!.length, query: m[2]! } : null;
}

/** Commands matching `query`: names starting with it first, then containing it. */
export function matchCommands(commands: readonly SessionCommand[], query: string): SessionCommand[] {
  const q = query.toLowerCase();
  const starts = commands.filter((c) => c.name.toLowerCase().startsWith(q));
  const contains = commands.filter((c) => !c.name.toLowerCase().startsWith(q) && c.name.toLowerCase().includes(q));
  return [...starts, ...contains];
}

/**
 * Commands whose real screen is the agent's CLI: over ACP they only reply in
 * text. Desktop runs them in an embedded terminal; the phone has none.
 */
const CLI_SCREEN_COMMANDS: ReadonlySet<string> = new Set(["mcp", "model", "config", "status"]);

/**
 * What the menu offers: the session's commands, or before its agent lists
 * them, the ones it listed in its last session here.
 */
export function offeredCommands(
  session: readonly SessionCommand[] | null | undefined,
  agent: readonly SessionCommand[] | null | undefined,
): SessionCommand[] {
  return (session ?? agent ?? []).filter((c) => !CLI_SCREEN_COMMANDS.has(c.name));
}

/** The text with `/name` in place of the query, and the caret just after it and one space. */
export function insertCommand(
  text: string,
  caret: number,
  found: SlashQuery,
  name: string,
): { text: string; caret: number } {
  const rest = text.slice(caret);
  const insert = /^\s/.test(rest) ? `/${name}` : `/${name} `;
  const at = found.start + insert.length;
  return { text: text.slice(0, found.start) + insert + rest, caret: /^\s/.test(rest) ? at + 1 : at };
}
