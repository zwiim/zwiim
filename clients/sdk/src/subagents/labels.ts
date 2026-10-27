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

/** "840 tokens", "9.9k tokens", "75k tokens", "1.2M tokens". */
export function tokensText(n: number): string {
  const short = (v: number, unit: string) => `${v < 10 ? Math.round(v * 10) / 10 : Math.round(v)}${unit}`;
  const count = n < 1000 ? String(Math.round(n)) : n < 999_500 ? short(n / 1000, "k") : short(n / 1_000_000, "M");
  return `${count} tokens`;
}

/** "3 tool calls", "2 sub-agents", "2 tool calls, 1 sub-agent": what a folded run holds. */
export function runText(tools: number, subagents: number): string {
  const calls = `${tools} ${tools === 1 ? "tool call" : "tool calls"}`;
  const agents = `${subagents} ${subagents === 1 ? "sub-agent" : "sub-agents"}`;
  if (subagents === 0) return calls;
  return tools === 0 ? agents : `${calls}, ${agents}`;
}
