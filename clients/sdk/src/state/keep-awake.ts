// Why the host's Mac stays awake, or will sleep anyway, as one reason every
// client words its own way.

import type { KeepAwakeStatus } from "../types/protocol";

/** One reason per thing a person can be told about their Mac's sleep. */
export type Reachability =
  | "unsupported"
  | "off"
  | "low_battery"
  | "on_battery"
  | "thermal"
  | "held"
  | "while_working"
  | "not_held";

/** The reason that applies first, in the host's own order. */
export function reachability(s: KeepAwakeStatus): Reachability {
  if (!s.supported) return "unsupported";
  if (s.mode === "off") return "off";
  // While working, the host blocks only once a turn starts; say so before.
  const battery = s.power.source === "battery";
  const percent = s.power.battery_percent;
  // Same order as the host: the 10% floor applies whatever the checkbox.
  if (s.blocked_by === "low_battery" || (battery && percent !== null && percent < 10)) return "low_battery";
  if (s.blocked_by === "on_battery" || (battery && !s.on_battery)) return "on_battery";
  if (s.blocked_by === "thermal") return "thermal";
  if (s.active) return "held";
  if (s.mode === "while_working") return "while_working";
  // Always, nothing in the way, yet no hold: the host tried and could not.
  return "not_held";
}
