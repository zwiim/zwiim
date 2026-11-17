import { describe, expect, it } from "vitest";
import {
  agentLock,
  heatLook,
  modeSegments,
  segmentAt,
  showAllLabel,
  travelPath,
  visibleModels,
} from "../src/sessions/picker";

const v = (n: number) => Array.from({ length: n }, (_, i) => ({ value: `m${i}`, label: `Model ${i}`, description: null }));

describe("visibleModels", () => {
  it("shows five, then all", () => {
    expect(visibleModels(v(8), { expanded: false, query: "" })).toMatchObject({ hidden: 3 });
    expect(visibleModels(v(8), { expanded: false, query: "" }).shown).toHaveLength(5);
    expect(visibleModels(v(8), { expanded: true, query: "" }).shown).toHaveLength(8);
  });

  it("searches every model, even hidden ones", () => {
    const found = visibleModels(v(8), { expanded: false, query: "model 7" });
    expect(found.shown.map((m) => m.value)).toEqual(["m7"]);
    expect(found.hidden).toBe(0);
  });

  it("returns to the first five once a search is cleared", () => {
    expect(visibleModels(v(8), { expanded: false, query: "  " }).shown).toHaveLength(5);
  });

  it("hides nothing at five or fewer", () => {
    expect(visibleModels(v(3), { expanded: false, query: "" })).toMatchObject({ hidden: 0 });
  });
});

it("labels show all", () => {
  expect(showAllLabel(5, false)).toBeNull();
  expect(showAllLabel(8, false)).toBe("Show all 8");
  expect(showAllLabel(8, true)).toBe("Show less");
});

describe("heatLook", () => {
  it("heats from calm to fire over any count", () => {
    expect(heatLook(0, 5).sparks).toBe(0);
    expect(heatLook(0, 5).shine).toBe(0);
    expect(heatLook(0, 5).text).toBe("#8fb8d8");
    expect(heatLook(4, 5).text).toBe("#ffc062");
    expect(heatLook(6, 7).text).toBe("#ffc062");
    expect(heatLook(3, 7).text).toBe("#a48bff");
  });

  it("has no NaN with one level", () => {
    const one = heatLook(0, 1);
    expect(one.text).toBe("#ffc062");
    expect(Number.isFinite(one.glow) && Number.isFinite(one.shine) && Number.isFinite(one.sparks)).toBe(true);
  });
});

it("travels through each level", () => {
  expect(travelPath(0, 2)).toEqual([1, 2]);
  expect(travelPath(4, 1)).toEqual([3, 2, 1]);
  expect(travelPath(2, 2)).toEqual([]);
});

it("snaps a drag inside the track, past either end too", () => {
  expect(segmentAt(-40, 300, 5)).toBe(0);
  expect(segmentAt(150, 300, 5)).toBe(2);
  expect(segmentAt(999, 300, 5)).toBe(4);
  expect(segmentAt(10, 300, 1)).toBe(0);
});

it("offers three modes with Auto for unattended", () => {
  const s = modeSegments({ current: "normal", label: "Default", available: ["normal", "unattended"] });
  expect(s.map((x) => x.label)).toEqual(["Ask first", "Plan", "Auto"]);
  expect(s.map((x) => x.mode)).toEqual(["normal", "plan", "unattended"]);
  expect(s.map((x) => x.enabled)).toEqual([true, false, true]);
  expect(modeSegments(null)).toEqual([]);
});

it("lets only an unsent fork switch agent", () => {
  expect(agentLock({ canSetAgent: true, forkedFrom: true, sent: false })).toBe("switchable");
  expect(agentLock({ canSetAgent: false, forkedFrom: true, sent: false })).toBe("locked");
  expect(agentLock({ canSetAgent: true, forkedFrom: true, sent: true })).toBe("locked");
  expect(agentLock({ canSetAgent: true, forkedFrom: false, sent: false })).toBe("locked");
});
