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

const listeners = new Set<() => void>();

function write(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage full or unavailable: preferences are best effort.
  }
  for (const listener of listeners) listener();
}

/** Called after any preference is written here; returns the unsubscribe. */
export function onPrefsChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
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

/** What the sidebar folds away: every workspace, and repositories whose worktrees are hidden. */
export interface SidebarFolds {
  workspaces: boolean;
  repositories: string[];
}

export function sidebarFolds(): SidebarFolds {
  const v = read<Partial<SidebarFolds>>("folds", {});
  return {
    workspaces: v.workspaces === true,
    repositories: Array.isArray(v.repositories) ? v.repositories.filter((id) => typeof id === "string") : [],
  };
}

export function rememberSidebarFolds(folds: SidebarFolds): void {
  write("folds", folds);
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

/** How big the whole window draws, as its zoom factor. */
export const ZOOM_LEVELS = [0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5] as const;

export function zoom(): number {
  const v = read<unknown>("zoom", 1);
  return ZOOM_LEVELS.find((z) => z === v) ?? 1;
}

export function rememberZoom(value: number): void {
  write("zoom", ZOOM_LEVELS.find((z) => z === value) ?? 1);
}

/** The longest font name kept. */
export const MAX_FONT_NAME = 64;

/** A font family's name as one CSS string can hold it: no quotes, backslashes or line breaks. */
export function cleanFontName(name: string): string {
  return name.replace(/["\\\r\n]/g, "").trim().slice(0, MAX_FONT_NAME);
}

/** The fonts code and the terminal use, by family name; empty for the app's own. */
export interface Fonts {
  code: string;
  terminal: string;
  /** The terminal's text size, in pixels. */
  terminalSize: number;
}

export const TERMINAL_SIZE_MIN = 9;
export const TERMINAL_SIZE_MAX = 24;
export const TERMINAL_SIZE_DEFAULT = 12;

export function fonts(): Fonts {
  const v = read<Partial<Fonts>>("fonts", {});
  const size = typeof v.terminalSize === "number" ? Math.round(v.terminalSize) : TERMINAL_SIZE_DEFAULT;
  return {
    code: typeof v.code === "string" ? cleanFontName(v.code) : "",
    terminal: typeof v.terminal === "string" ? cleanFontName(v.terminal) : "",
    terminalSize: Math.min(TERMINAL_SIZE_MAX, Math.max(TERMINAL_SIZE_MIN, size)),
  };
}

export function rememberFonts(patch: Partial<Fonts>): void {
  write("fonts", { ...fonts(), ...patch });
}

/** Whether code draws a font's ligatures, `!=` and `=>` as one glyph each. */
export function codeLigatures(): boolean {
  return read<unknown>("code_ligatures", true) !== false;
}

export function rememberCodeLigatures(on: boolean): void {
  write("code_ligatures", on);
}

/** Whether the dark theme draws with less contrast: lifted greys, dimmer text. */
export function softDark(): boolean {
  return read<unknown>("soft_dark", false) === true;
}

export function rememberSoftDark(on: boolean): void {
  write("soft_dark", on);
}

/** Done and failed in the usual green and red, or in colors told apart without red-green or blue-yellow vision. */
export type ColorVision = "default" | "red_green" | "blue_yellow";

export function colorVision(): ColorVision {
  const v = read<unknown>("color_vision", "default");
  return v === "red_green" || v === "blue_yellow" ? v : "default";
}

export function rememberColorVision(value: ColorVision): void {
  write("color_vision", value);
}

/** The colors code draws in: Zwiim's own, or a well-known theme in its light or dark variant. */
export type CodeTheme = "zwiim" | "github" | "one" | "catppuccin" | "vitesse" | "solarized" | "rose-pine";

const CODE_THEME_IDS: readonly CodeTheme[] = ["zwiim", "github", "one", "catppuccin", "vitesse", "solarized", "rose-pine"];

export function codeTheme(): CodeTheme {
  const v = read<unknown>("code_theme", "zwiim");
  return CODE_THEME_IDS.find((t) => t === v) ?? "zwiim";
}

export function rememberCodeTheme(value: CodeTheme): void {
  write("code_theme", value);
}

/** How a markdown file's preview reads: like the transcript, or set as a book page. */
export type MarkdownStyle = "default" | "book";

export function markdownStyle(): MarkdownStyle {
  return read<unknown>("markdown_style", "default") === "book" ? "book" : "default";
}

export function rememberMarkdownStyle(value: MarkdownStyle): void {
  write("markdown_style", value);
}

/** How a changed file reads: one column, or old and new side by side. */
export type DiffLayout = "unified" | "split";

export function diffLayout(): DiffLayout {
  return read<unknown>("diff_layout", "unified") === "split" ? "split" : "unified";
}

export function rememberDiffLayout(value: DiffLayout): void {
  write("diff_layout", value);
}

/** Which key sends a message: Enter (Shift+Enter for a new line), or Mod+Enter (Enter for a new line). */
export type SendKey = "enter" | "mod_enter";

export function sendKey(): SendKey {
  return read<unknown>("send_key", "enter") === "mod_enter" ? "mod_enter" : "enter";
}

export function rememberSendKey(value: SendKey): void {
  write("send_key", value);
}

/** What sending does while an agent works: wait for its turn to end, or go into the turn. */
export type FollowUpMode = "queue" | "steer";

export function followUpMode(): FollowUpMode {
  return read<unknown>("follow_up_mode", "queue") === "steer" ? "steer" : "queue";
}

export function rememberFollowUpMode(value: FollowUpMode): void {
  write("follow_up_mode", value);
}

/** Whether a long paste into a composer goes with the message as a text file. */
export function autoConvertLongText(): boolean {
  return read<unknown>("auto_convert_long_text", true) !== false;
}

export function rememberAutoConvertLongText(on: boolean): void {
  write("auto_convert_long_text", on);
}

/** Whether the context ring shows all the time; off, only once the window is nearly full. */
export function alwaysShowContext(): boolean {
  return read<unknown>("always_show_context", true) !== false;
}

export function rememberAlwaysShowContext(on: boolean): void {
  write("always_show_context", on);
}

/** Whether a reply shows every tool call on its own row and its thinking open, folding nothing. */
export function expandToolCalls(): boolean {
  return read<unknown>("expand_tool_calls", false) === true;
}

export function rememberExpandToolCalls(on: boolean): void {
  write("expand_tool_calls", on);
}

/** Whether a pull request waiting on checks offers Auto-merge. Off leaves plain Merge. */
export function autoMergeButton(): boolean {
  return read<unknown>("auto_merge_button", true) !== false;
}

export function rememberAutoMergeButton(on: boolean): void {
  write("auto_merge_button", on);
}

/** What happens when an agent finishes a turn. */
export interface TurnAlerts {
  /** A system notification, while this window is not in front. */
  notify: boolean;
  sound: boolean;
}

export function turnAlerts(): TurnAlerts {
  const v = read<Partial<TurnAlerts>>("turn_alerts", {});
  return { notify: v.notify !== false, sound: v.sound === true };
}

export function rememberTurnAlerts(alerts: Partial<TurnAlerts>): void {
  write("turn_alerts", { ...turnAlerts(), ...alerts });
}
