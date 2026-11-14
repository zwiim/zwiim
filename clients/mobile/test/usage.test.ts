import type { CloudUsage } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { limitSheet, resetDay, usageLine } from "../src/account/usage";

const GB = 1024 ** 3;
const uk = { locale: "en-GB", timeZone: "UTC" };
// 12 Dec 2026, 00:00 UTC.
const resets_at = Date.UTC(2026, 11, 12) / 1000;

const usage = (browserUsed: number, browserLimit: number | null = 5 * GB): CloudUsage => ({
  plan: { id: "free", name: "Free" },
  period_start: Date.UTC(2026, 10, 12) / 1000,
  resets_at,
  channels: {
    browser: { used: browserUsed, limit: browserLimit },
    total: { used: browserUsed, limit: 10 * GB },
  },
});

describe("usageLine", () => {
  it("shows from 80% of the browser limit", () => {
    expect(usageLine(usage(3.9 * GB), uk)).toBeNull();
    expect(usageLine(usage(4 * GB), uk)).toBe("Free · 4 of 5 GB used · resets 12 Dec");
    expect(usageLine(usage(4.35 * GB), uk)).toBe("Free · 4.4 of 5 GB used · resets 12 Dec");
  });

  it("never reads past the limit", () => {
    expect(usageLine(usage(6 * GB), uk)).toBe("Free · 5 of 5 GB used · resets 12 Dec");
  });

  it("shows nothing unlimited, without a plan, or without usage", () => {
    expect(usageLine(usage(100 * GB, null), uk)).toBeNull();
    expect(usageLine({ plan: null }, uk)).toBeNull();
    expect(usageLine(null, uk)).toBeNull();
  });
});

describe("resetDay", () => {
  it("uses the person's format and time zone", () => {
    expect(resetDay(resets_at, uk)).toBe("12 Dec");
    expect(resetDay(resets_at, { locale: "en-US", timeZone: "UTC" })).toBe("Dec 12");
    // Still 11 Dec in California.
    expect(resetDay(resets_at, { locale: "en-GB", timeZone: "America/Los_Angeles" })).toBe("11 Dec");
  });
});

describe("limitSheet", () => {
  it("says what stopped, when it resets and what still works", () => {
    expect(limitSheet("browser", usage(5 * GB), "MacBook Pro", uk)).toEqual({
      title: "Browser limit reached",
      lines: [
        "You've used 5 GB of remote browsing this period.",
        "It resets on 12 Dec.",
        "Chat, terminal and approvals still work.",
        "On the same Wi-Fi as MacBook Pro, the browser works as usual.",
      ],
    });
    expect(limitSheet("total", usage(5 * GB), "MacBook Pro", uk)).toEqual({
      title: "Remote limit reached",
      lines: ["Phone access over the internet resets on 12 Dec.", "On the same Wi-Fi as MacBook Pro, everything works."],
    });
  });

  it("leaves out what the Cloud didn't say", () => {
    expect(limitSheet("browser", null, "Mac", uk).lines.slice(0, 2)).toEqual([
      "You've used this period's remote browsing.",
      "Chat, terminal and approvals still work.",
    ]);
    expect(limitSheet("total", null, "Mac", uk).lines[0]).toBe("Phone access over the internet is paused for this period.");
  });
});
