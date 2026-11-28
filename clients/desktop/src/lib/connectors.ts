// What the connectors dialog shows, worked out from what the Host reports.
//
// Each tool has one control: Off, Ask or Allow. Off is the connector's own
// switch (`enabled`), the same in every workspace; Ask and Allow are the row
// on the grant shown, the one for this workspace or else the one for every
// workspace. A grant made for one agent is left alone here.

import type { Connector, ConnectorExecution, ConnectorGrant, ConnectorTool, WorkspaceInfo } from "@zwiim/sdk";

export type ToolChoice = "off" | "ask" | "allow";
export type ToolGroup = "read" | "write" | "destructive";
export type Scope = "nowhere" | "workspace" | "all";

/** Limits are set in these steps, from MIN_LIMIT to MAX_LIMIT. */
export const LIMIT_STEP = 500;
export const MIN_LIMIT = 1000;
export const MAX_LIMIT = 10000;
/** Above this across the connectors a session gets, the user is warned (r3 §8.2). Nothing is held back. */
export const SESSION_WARNING = 12000;

/** The root workspace a worktree belongs to: grants name roots. */
export function rootOf(workspaces: readonly WorkspaceInfo[] | null | undefined, workspaceId: string): string {
  return workspaces?.find((w) => w.workspace_id === workspaceId)?.parent_workspace_id ?? workspaceId;
}

/** The grant the dialog shows and edits: this workspace's, else every workspace's. */
export function shownGrant(grants: readonly ConnectorGrant[], root: string): ConnectorGrant | null {
  const anyAgent = grants.filter((g) => g.agent === null);
  return anyAgent.find((g) => g.workspace_id === root) ?? anyAgent.find((g) => g.workspace_id === null) ?? null;
}

export function scopeOf(grant: ConnectorGrant | null): Scope {
  if (!grant) return "nowhere";
  return grant.workspace_id === null ? "all" : "workspace";
}

/** Connected and turned on: agents allowed it get its tools. */
export function usable(c: Connector): boolean {
  return c.status === "active" && c.enabled;
}

/**
 * About what the tools that are on take of a session's context in `root`,
 * across every connector that is on and allowed there. An upper bound: a grant for
 * one agent, or a tool a grant denies, can leave some out.
 */
export function sessionCost(
  connectors: readonly Connector[],
  grants: Readonly<Record<string, readonly ConnectorGrant[]>>,
  root: string,
): number {
  return connectors
    .filter((c) => usable(c) && allowedHere(grants[c.connector_id] ?? [], root))
    .reduce((sum, c) => sum + c.tools_used, 0);
}

/** Whether a session in `root` gets the connector, by any grant. */
export function allowedHere(grants: readonly ConnectorGrant[], root: string): boolean {
  return grants.some((g) => g.workspace_id === null || g.workspace_id === root);
}

export function toolChoice(tool: ConnectorTool, grant: ConnectorGrant | null): ToolChoice {
  if (!tool.enabled) return "off";
  const row = grant?.tools.find((r) => r.name === tool.name);
  if (row?.policy === "deny") return "off";
  if (row?.policy === "allow" && !row.changed) return "allow";
  return "ask";
}

/** An `allow` given for an earlier version of the tool: it asks again. */
export function changed(tool: ConnectorTool, grant: ConnectorGrant | null): boolean {
  return grant?.tools.some((r) => r.name === tool.name && r.changed) ?? false;
}

/** By the server's own hints; a tool that says nothing counts as a write. */
export function toolGroup(tool: ConnectorTool): ToolGroup {
  const hints = tool.annotations ?? {};
  if (hints.readOnlyHint === true) return "read";
  if (hints.destructiveHint === true) return "destructive";
  return "write";
}

/** The smallest limit that fits `needs` more, or null when none does. */
export function limitThatFits(needs: number, used: number): number | null {
  const limit = Math.max(MIN_LIMIT, Math.ceil((used + needs) / LIMIT_STEP) * LIMIT_STEP);
  return limit > MAX_LIMIT ? null : limit;
}

/** Every limit offered, from MIN_LIMIT to MAX_LIMIT. */
export function limitSteps(): number[] {
  const steps = [];
  for (let n = MIN_LIMIT; n <= MAX_LIMIT; n += LIMIT_STEP) steps.push(n);
  return steps;
}

export type Tone = "ok" | "brand" | "bad" | "faint";

/** The connector's state in a few words, and its dot's colour. */
export function statusLine(c: Connector): { text: string; tone: Tone } {
  switch (c.status) {
    case "active":
      return c.enabled ? { text: "connected", tone: "ok" } : { text: "turned off", tone: "faint" };
    case "needs_reauth":
      return { text: "needs a key", tone: "brand" };
    case "error":
      return { text: "can't reach server", tone: "bad" };
    case "validating":
      return { text: "checking", tone: "faint" };
    case "revoking":
    case "revoked":
      return { text: "disconnecting", tone: "faint" };
  }
}

/** How a call through a connector went, in a few words, and its colour. */
export function callOutcome(e: ConnectorExecution): { text: string; tone: Tone } {
  switch (e.state) {
    case "received":
    case "dispatched":
      return { text: "running", tone: "faint" };
    case "awaiting_approval":
      return { text: "waiting for you", tone: "brand" };
    case "succeeded":
      return { text: "ran", tone: "ok" };
    case "tool_error":
      return { text: "tool error", tone: "bad" };
    case "failed":
      return { text: "failed", tone: "bad" };
    case "outcome_unknown":
      return { text: "may have run", tone: "brand" };
    case "approval_denied":
      return { text: "you denied", tone: "faint" };
    case "approval_expired":
      return { text: "no answer", tone: "faint" };
    case "approval_void":
      return { text: "approval no longer valid", tone: "faint" };
    case "cancelled_before_dispatch":
      return { text: "cancelled", tone: "faint" };
    case "rejected":
      return { text: REJECTED[e.reason ?? ""] ?? "not sent", tone: "faint" };
  }
}

const REJECTED: Record<string, string> = {
  unavailable: "connector unavailable",
  not_exposed: "tool off",
  not_sent: "not sent",
  denied: "not allowed",
};

/** "4,000": sizes and limits, grouped in thousands. */
export const count = (n: number) => n.toLocaleString("en-US");

/** A name to start the add form with: `https://mcp.linear.app/mcp` → `Linear`. */
export function guessName(url: string): string {
  let host: string;
  try {
    host = new URL(url).hostname;
  } catch {
    return "";
  }
  // An address names no service: leave the name to the user.
  if (host === "localhost" || host.startsWith("[") || /^\d+(\.\d+){3}$/.test(host)) return "";
  const part = host.split(".").find((p) => !["mcp", "api", "www"].includes(p)) ?? "";
  return part.charAt(0).toUpperCase() + part.slice(1);
}

/**
 * A server on this machine or network: the Host reaches it as `local`,
 * which also allows plain http. Everything else is `public`, https only.
 */
export function isLocalUrl(url: string): boolean {
  let host: string;
  try {
    host = new URL(url).hostname.replace(/^\[|\]$/g, "");
  } catch {
    return false;
  }
  if (host === "localhost" || host === "::1" || host.endsWith(".local")) return true;
  const ip = host.split(".").map(Number);
  if (ip.length !== 4 || ip.some((n) => !Number.isInteger(n))) return false;
  const [a, b] = ip as [number, number, number, number];
  return a === 127 || a === 10 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31);
}
