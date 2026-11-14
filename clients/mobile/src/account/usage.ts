// What the phone says about the account's relay limits (browser design 15.6,
// DESIGN.md Global States): the usage line from 80% of the browser limit, and
// the sheets at the browser and total limits. Only over the relay: on the
// same network nothing is limited.

import type { CloudChannelUsage, CloudUsage } from "@zwiim/sdk";

/** Which limit a sheet is about. */
export type RelayLimit = "browser" | "total";

/** How the person reads dates: their locale and time zone unless a test says. */
export interface DateStyle {
  locale?: string;
  timeZone?: string;
}

const GB = 1024 ** 3;

/** `4`, `3.2`, `12`: whole numbers from 10 GB, one decimal below. */
function gb(bytes: number): string {
  const n = bytes / GB;
  return n >= 10 ? String(Math.round(n)) : String(Math.round(n * 10) / 10);
}

/** `12 Dec` (or `Dec 12`): the reset day in the person's own format. */
export function resetDay(resetsAt: number, style: DateStyle = {}): string {
  return new Date(resetsAt * 1000).toLocaleDateString(style.locale, {
    day: "numeric",
    month: "short",
    timeZone: style.timeZone,
  });
}

function limited(c: CloudChannelUsage | undefined): c is { used: number; limit: number } {
  return !!c && c.limit !== null && c.limit > 0;
}

/**
 * The browser toolbar's line, "Free · 4 of 5 GB used · resets 12 Dec", from
 * 80% of the browser limit. Null below it, without a plan, or unlimited.
 */
export function usageLine(usage: CloudUsage | null, style: DateStyle = {}): string | null {
  const browser = usage?.channels?.browser;
  if (!usage?.plan || !limited(browser) || usage.resets_at === undefined) return null;
  if (browser.used < browser.limit * 0.8) return null;
  const used = Math.min(browser.used, browser.limit);
  return `${usage.plan.name} · ${gb(used)} of ${gb(browser.limit)} GB used · resets ${resetDay(usage.resets_at, style)}`;
}

/**
 * A limit sheet's title and lines. Without usage (the Cloud didn't answer)
 * it leaves out the amount and the reset day rather than guess them.
 */
export function limitSheet(
  limit: RelayLimit,
  usage: CloudUsage | null,
  hostName: string,
  style: DateStyle = {},
): { title: string; lines: string[] } {
  const resets = usage?.resets_at !== undefined ? resetDay(usage.resets_at, style) : null;
  if (limit === "browser") {
    const browser = usage?.channels?.browser;
    return {
      title: "Browser limit reached",
      lines: [
        limited(browser)
          ? `You've used ${gb(browser.limit)} GB of remote browsing this period.`
          : "You've used this period's remote browsing.",
        ...(resets ? [`It resets on ${resets}.`] : []),
        "Chat, terminal and approvals still work.",
        `On the same Wi-Fi as ${hostName}, the browser works as usual.`,
      ],
    };
  }
  return {
    title: "Remote limit reached",
    lines: [
      resets ? `Phone access over the internet resets on ${resets}.` : "Phone access over the internet is paused for this period.",
      `On the same Wi-Fi as ${hostName}, everything works.`,
    ],
  };
}
