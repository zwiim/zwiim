// Words both clients use for sub-agents and the background work the agent
// waits on, so Desktop and Mobile say the same thing.

/** Launch titles that only say "a sub-agent", not what it does. */
const GENERIC_LAUNCH = new Set(["agent", "task", "subagent", "spawnagent", "spawnsubagent"]);

/**
 * What a sub-agent's card says it does: its description, else its launch's
 * title (without code quotes) when that says more than "Agent" or "Task";
 * else null, and the card shows nothing after the model.
 */
export function subagentDescription(
  description: string | null | undefined,
  title: string | null | undefined,
): string | null {
  const said = description?.trim();
  if (said) return said;
  let t = title?.trim() ?? "";
  if (t.length > 1 && t.startsWith("`") && t.endsWith("`")) t = t.slice(1, -1).trim();
  if (!t || GENERIC_LAUNCH.has(t.toLowerCase().replace(/[\s_-]/g, ""))) return null;
  return t;
}

/** "Waiting on 1 background task", "Waiting on 3 background tasks". */
export function waitingText(count: number): string {
  return `Waiting on ${count} background ${count === 1 ? "task" : "tasks"}`;
}
