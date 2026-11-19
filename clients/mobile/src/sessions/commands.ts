import { CLI_SCREEN_COMMANDS, type SessionCommand, type SlashQuery } from "@zwiim/sdk";

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

/** Commands listed over the composer; more open a searchable sheet. */
export const COMMAND_PREVIEW = 5;

/** The matches to list inline, and how many there are when some only show in the sheet. */
export function commandPreview(matches: readonly SessionCommand[]): { shown: SessionCommand[]; more: number | null } {
  if (matches.length <= COMMAND_PREVIEW) return { shown: [...matches], more: null };
  return { shown: matches.slice(0, COMMAND_PREVIEW), more: matches.length };
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
