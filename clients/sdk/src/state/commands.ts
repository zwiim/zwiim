// Slash commands in a composer: finding the `/query` being typed and the
// agent's commands that match it. Shared so desktop and phone offer the same.

import type { SessionCommand } from "../types/protocol";

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
 * Commands the agents list whose real screen is their CLI's: over ACP they
 * only reply in text. Desktop runs them in an embedded terminal; the phone,
 * which has none, leaves them out.
 */
export const CLI_SCREEN_COMMANDS: ReadonlySet<string> = new Set(["mcp", "model", "config", "status"]);
