import { describe, expect, it } from "vitest";
import type { KeepAwakeStatus } from "@zwiim/sdk";
import type { HostConnectionView } from "../src/host/connection";
import { reachabilityText } from "../src/host/status";

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

const via = (active: HostConnectionView["active"]) => ({ active }) as HostConnectionView;
const live = via("lan");

describe("reachabilityText", () => {
  it("shows nothing until the Mac says, or where it cannot keep awake", () => {
    expect(reachabilityText(null, live)).toBeNull();
    expect(reachabilityText(status({ supported: false }), live)).toBeNull();
  });

  it("says whether the Mac stays reachable, and why not", () => {
    const cases: [Partial<KeepAwakeStatus>, string, string][] = [
      [{ active: true }, "Mac kept awake", "done"],
      [{ mode: "while_working", active: false }, "Mac kept awake while agents work", "idle"],
      [{ mode: "off", active: false }, "Mac sleeps as usual", "idle"],
      [{ active: false, blocked_by: "on_battery" }, "Mac on battery: may sleep", "idle"],
      [{ active: false, blocked_by: "low_battery" }, "Mac battery below 10%: may sleep", "idle"],
      [{ active: false, blocked_by: "thermal" }, "Mac too hot: may sleep", "idle"],
      [{ mode: "always", active: false }, "Could not keep the Mac awake", "idle"],
    ];
    for (const [over, line, dot] of cases) {
      const r = reachabilityText(status(over), live);
      expect([over, r?.line, r?.dot]).toEqual([over, line, dot]);
      expect(r?.warnings).toEqual([]);
    }
  });

  it("lists the Mac's warnings in the order given", () => {
    expect(reachabilityText(status({ warnings: ["lid_close_will_sleep", "on_battery_will_drain"] }), live)?.warnings).toEqual([
      "Closing the lid will sleep the Mac",
      "Running on battery",
    ]);
    expect(reachabilityText(status({ warnings: ["on_battery_will_drain", "lid_close_will_sleep"] }), live)?.warnings).toEqual([
      "Running on battery",
      "Closing the lid will sleep the Mac",
    ]);
  });

  it("shows nothing while the Mac is out of reach, rather than what it last said", () => {
    expect(reachabilityText(status(), via("relay"))?.line).toBe("Mac kept awake");
    for (const active of ["connecting", "reconnecting", "offline"] as HostConnectionView["active"][]) {
      expect([active, reachabilityText(status(), via(active))]).toEqual([active, null]);
    }
    expect(reachabilityText(status(), null)).toBeNull();
  });
});
