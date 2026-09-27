import { describe, expect, it } from "vitest";
import { initialSessionView, reduceSession } from "../src/state/session";
import { contextFraction, contextLeft } from "../src/state/usage";
import type { SessionSnapshot, TurnUsage } from "../src/types/protocol";

const snapshot = (over: Partial<SessionSnapshot> = {}): SessionSnapshot => ({
  status: "connected",
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
  ...over,
});

const turn: TurnUsage = {
  total_tokens: 1200,
  input_tokens: 1000,
  output_tokens: 200,
  thought_tokens: null,
  cached_read_tokens: 800,
  cached_write_tokens: null,
};

describe("session usage state", () => {
  it("starts unreported and follows the snapshot and live usage events", () => {
    let v = initialSessionView("s1");
    expect(v.usage).toBeNull();
    expect(v.turnUsage).toBeNull();
    v = reduceSession(v, { type: "snapshot", epoch: "e", seq: 1, snapshot: snapshot() });
    expect(v.usage).toBeNull();
    // A host without turn usage on its snapshot reads as not reported.
    expect(v.turnUsage).toBeNull();
    v = reduceSession(v, {
      type: "event",
      event: { session_id: "s1", seq: 2, type: "session.usage", payload: { used: 5000, size: 200000 } },
    });
    expect(v.usage).toEqual({ used: 5000, size: 200000 });
    v = reduceSession(v, {
      type: "event",
      event: { session_id: "s1", seq: 3, type: "session.turn_usage", payload: turn },
    });
    expect(v.turnUsage).toEqual(turn);
    // The last report stays on screen once the agent goes away.
    v = reduceSession(v, { type: "detached", reason: "session_not_live" });
    expect(v.usage).toEqual({ used: 5000, size: 200000 });
    expect(v.turnUsage).toEqual(turn);
  });

  it("replays usage events and takes the snapshot's figures on attach", () => {
    let v = reduceSession(initialSessionView("s1"), {
      type: "snapshot",
      epoch: "e",
      seq: 1,
      snapshot: snapshot({ usage: { used: 10, size: 258400 }, turn_usage: turn }),
    });
    expect(v.usage).toEqual({ used: 10, size: 258400 });
    expect(v.turnUsage?.cached_read_tokens).toBe(800);
    v = reduceSession(v, {
      type: "replay",
      epoch: "e",
      events: [{ session_id: "s1", seq: 2, type: "session.usage", payload: { used: 40, size: 258400 } }],
      high_water_seq: 2,
    });
    expect(v.usage?.used).toBe(40);
  });
});

describe("context figures", () => {
  it("derive only from what the agent reported", () => {
    expect(contextLeft({ used: 50000, size: 200000 })).toBe(150000);
    expect(contextFraction({ used: 50000, size: 200000 })).toBe(0.25);
    for (const usage of [null, undefined, { used: null, size: 200000 }, { used: 10, size: null }, { used: 10, size: 0 }]) {
      expect(contextLeft(usage)).toBeNull();
      expect(contextFraction(usage)).toBeNull();
    }
    // Over the window reads as full, never negative.
    expect(contextLeft({ used: 210000, size: 200000 })).toBe(0);
    expect(contextFraction({ used: 210000, size: 200000 })).toBe(1);
  });
});
