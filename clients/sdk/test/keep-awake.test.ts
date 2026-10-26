import { describe, expect, it } from "vitest";
import { reachability, type Reachability } from "../src/state/keep-awake";
import type { KeepAwakeStatus } from "../src/types/protocol";

const status = (over: Partial<KeepAwakeStatus> = {}): KeepAwakeStatus => ({
  supported: true,
  mode: "always",
  on_battery: false,
  lid_closed: { support: "no_lid", enabled: false },
  active: true,
  blocked_by: null,
  warnings: [],
  power: { source: "ac", battery_percent: null },
  ...over,
});

describe("reachability", () => {
  it("says why the Mac will sleep, in the host's order", () => {
    const cases: [Partial<KeepAwakeStatus>, Reachability][] = [
      [{ supported: false, active: false }, "unsupported"],
      [{ mode: "off", active: false }, "off"],
      [{ active: false, blocked_by: "low_battery" }, "low_battery"],
      // The host checks the 10% floor first; so does the reason.
      [{ active: false, blocked_by: "low_battery", power: { source: "battery", battery_percent: 5 } }, "low_battery"],
      [{ active: false, blocked_by: "on_battery" }, "on_battery"],
      [{ active: false, blocked_by: "thermal" }, "thermal"],
      [{ active: true }, "held"],
      [{ mode: "while_working", active: false }, "while_working"],
      [{ mode: "always", active: false }, "not_held"],
      // While working, nothing is blocked until a turn starts; say so before.
      [{ mode: "while_working", active: false, power: { source: "battery", battery_percent: 80 } }, "on_battery"],
      [
        { mode: "while_working", active: false, on_battery: true, power: { source: "battery", battery_percent: 9 } },
        "low_battery",
      ],
    ];
    for (const [over, reason] of cases) {
      expect([over, reachability(status(over))]).toEqual([over, reason]);
    }
  });
});
