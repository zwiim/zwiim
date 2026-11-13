// The Host browser's tabs and engine, as the `browser.subscribe` frames
// report them. Every event carries a tab's whole state, so applying one the
// snapshot already holds changes nothing.

import type { BrowserEngine, BrowserFrame, BrowserTab, BrowserUiRequest, WireError } from "../types/protocol";

/** One step of the browser feed: the subscribe going out, or a frame back. */
export type BrowserUpdate = { type: "browser.subscribing" } | BrowserFrame;

export interface BrowserState {
  /**
   * `off` until a view asks for the browser; `loading` until the snapshot
   * arrives (again after each reconnect); `refused` when the Host said no.
   */
  status: "off" | "loading" | "live" | "refused";
  /** Every tab, in each owner's order. Kept across a reconnect until the next snapshot. */
  tabs: BrowserTab[];
  engine: BrowserEngine | null;
  /** What tabs wait on a person for (dialogs), oldest first. */
  uiRequests: BrowserUiRequest[];
  error: WireError | null;
}

export function initialBrowserState(): BrowserState {
  return { status: "off", tabs: [], engine: null, uiRequests: [], error: null };
}

export function reduceBrowser(state: BrowserState, update: BrowserUpdate): BrowserState {
  switch (update.type) {
    case "browser.subscribing":
      return { ...state, status: "loading", error: null };
    case "browser.snapshot":
      return {
        status: "live",
        tabs: ordered(update.tabs),
        engine: update.engine,
        uiRequests: update.ui_requests ?? [],
        error: null,
      };
    case "browser.tab.state": {
      const tabs = state.tabs.filter((t) => t.tab_id !== update.tab.tab_id);
      return { ...state, tabs: ordered([...tabs, update.tab]) };
    }
    case "browser.tab.removed": {
      if (!state.tabs.some((t) => t.tab_id === update.tab_id)) return state;
      return { ...state, tabs: state.tabs.filter((t) => t.tab_id !== update.tab_id) };
    }
    case "browser.engine.state":
      return { ...state, engine: update.engine };
    case "browser.ui.request": {
      const others = state.uiRequests.filter((r) => r.request_id !== update.request.request_id);
      return { ...state, uiRequests: [...others, update.request] };
    }
    case "browser.ui.done": {
      if (!state.uiRequests.some((r) => r.request_id === update.request_id)) return state;
      return { ...state, uiRequests: state.uiRequests.filter((r) => r.request_id !== update.request_id) };
    }
    case "browser.error":
      return { ...state, status: "refused", error: update.error };
  }
}

/** A session's tabs, in order. */
export function sessionTabs(state: BrowserState, session_id: string): BrowserTab[] {
  return state.tabs.filter((t) => t.owner.kind === "session" && t.owner.session_id === session_id);
}

/** The person's own tabs, in order. */
export function personalTabs(state: BrowserState): BrowserTab[] {
  return state.tabs.filter((t) => t.owner.kind === "personal");
}

/** What the tab `tab_id` waits on, oldest first. */
export function tabUiRequests(state: BrowserState, tab_id: string): BrowserUiRequest[] {
  return state.uiRequests.filter((r) => r.tab_id === tab_id);
}

function ordered(tabs: BrowserTab[]): BrowserTab[] {
  return [...tabs].sort((a, b) => a.position - b.position);
}
