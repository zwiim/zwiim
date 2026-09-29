// Client preferences: what this Desktop last showed and how its window was
// laid out. Local to this client and never authoritative — a remembered
// workspace or session the host no longer lists is simply ignored.

import type { ConnectionMode } from "@zwiim/sdk";

const PREFIX = "zwiim.desktop.";

function read<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(PREFIX + key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage full or unavailable: preferences are best effort.
  }
}

export interface LastSelection {
  workspaceId: string | null;
  sessionId: string | null;
}

/** Per host: a different host has different workspaces and sessions. */
export function lastSelection(hostId: string): LastSelection {
  const v = read<Partial<LastSelection>>(`last.${hostId}`, {});
  return {
    workspaceId: typeof v.workspaceId === "string" ? v.workspaceId : null,
    sessionId: typeof v.sessionId === "string" ? v.sessionId : null,
  };
}

export function rememberSelection(hostId: string, selection: LastSelection): void {
  write(`last.${hostId}`, selection);
}

export const SIDEBAR_MIN = 200;
export const SIDEBAR_MAX = 480;
const SIDEBAR_DEFAULT = 256;

export interface SidebarLayout {
  width: number;
  collapsed: boolean;
}

export function sidebarLayout(): SidebarLayout {
  const v = read<Partial<SidebarLayout>>("sidebar", {});
  const width = typeof v.width === "number" ? v.width : SIDEBAR_DEFAULT;
  return {
    width: Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, width)),
    collapsed: v.collapsed === true,
  };
}

export function rememberSidebar(layout: SidebarLayout): void {
  write("sidebar", layout);
}

export const PANEL_MIN = 300;
export const PANEL_MAX = 720;
const PANEL_DEFAULT = 400;
export const TERMINAL_MIN = 0.2;
export const TERMINAL_MAX = 0.8;
const TERMINAL_DEFAULT = 0.4;

/** The right-hand panel: its width, and the terminal's share of its height. */
export interface PanelLayout {
  width: number;
  collapsed: boolean;
  terminal: number;
  terminalCollapsed: boolean;
}

export function panelLayout(): PanelLayout {
  const v = read<Partial<PanelLayout>>("panel", {});
  const width = typeof v.width === "number" ? v.width : PANEL_DEFAULT;
  const terminal = typeof v.terminal === "number" ? v.terminal : TERMINAL_DEFAULT;
  return {
    width: Math.min(PANEL_MAX, Math.max(PANEL_MIN, width)),
    collapsed: v.collapsed === true,
    terminal: Math.min(TERMINAL_MAX, Math.max(TERMINAL_MIN, terminal)),
    terminalCollapsed: v.terminalCollapsed === true,
  };
}

export function rememberPanel(layout: PanelLayout): void {
  write("panel", layout);
}

/** Model and effort last chosen for an agent; a new session starts from them if still offered. */
export interface AgentChoices {
  model: string | null;
  effort: string | null;
}

export function agentChoices(agent: string): AgentChoices {
  const v = read<Partial<AgentChoices>>(`agent.${agent}`, {});
  return {
    model: typeof v.model === "string" ? v.model : null,
    effort: typeof v.effort === "string" ? v.effort : null,
  };
}

export function rememberAgentChoices(agent: string, choices: Partial<AgentChoices>): void {
  const next = agentChoices(agent);
  if (choices.model) next.model = choices.model;
  if (choices.effort) next.effort = choices.effort;
  write(`agent.${agent}`, next);
}

/** The first-run choice between signing in and local use was made. */
export function welcomed(): boolean {
  return read<boolean>("welcomed", false) === true;
}

export function rememberWelcomed(): void {
  write("welcomed", true);
}

/** Which host this Desktop shows: this computer's, or one on another computer. */
export type HostChoice = { kind: "local" } | { kind: "remote"; hostId: string };

export function hostChoice(): HostChoice {
  const v = read<Partial<{ kind: string; hostId: unknown }>>("host", {});
  return v.kind === "remote" && typeof v.hostId === "string" ? { kind: "remote", hostId: v.hostId } : { kind: "local" };
}

export function rememberHostChoice(choice: HostChoice): void {
  write("host", choice);
}

/**
 * How a host on another computer is reached: directly on this network when
 * it is there (`auto`), only directly (`local`), or only through Zwiim Cloud
 * (`cloud`).
 */
export function connectionMode(): ConnectionMode {
  const v = read<unknown>("connection_mode", "auto");
  return v === "local" || v === "cloud" ? v : "auto";
}

export function rememberConnectionMode(mode: ConnectionMode): void {
  write("connection_mode", mode);
}

/** The window's look: the system's, or always light or dark. */
export type Theme = "system" | "light" | "dark";

export function theme(): Theme {
  const v = read<unknown>("theme", "system");
  return v === "light" || v === "dark" ? v : "system";
}

export function rememberTheme(value: Theme): void {
  write("theme", value);
}
