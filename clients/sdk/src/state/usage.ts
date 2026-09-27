// Derived context figures. Only arithmetic on what the agent reported:
// anything it left out stays null.

import type { Usage } from "../types/protocol";

/** Tokens left in the context window, when the agent reported both figures. */
export function contextLeft(usage: Usage | null | undefined): number | null {
  if (usage?.used == null || usage.size == null || usage.size <= 0) return null;
  return Math.max(0, usage.size - usage.used);
}

/** The share of the context window in use (0–1), when the agent reported both figures. */
export function contextFraction(usage: Usage | null | undefined): number | null {
  if (usage?.used == null || usage.size == null || usage.size <= 0) return null;
  return Math.min(1, usage.used / usage.size);
}
