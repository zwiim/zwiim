import type {
  HistoryTurn,
  OpName,
  OpResult,
  ServerFrame,
  SessionEvent,
  ToolCallStatus,
} from "../types/protocol";

const FRAME_TYPES = new Set([
  "snapshot",
  "replay",
  "event",
  "detached",
  "terminal",
  "host.snapshot",
  "host.event",
  "browser.snapshot",
  "browser.tab.state",
  "browser.tab.removed",
  "browser.engine.state",
  "browser.ui.request",
  "browser.ui.done",
  "browser.agent.action",
  "browser.ui.menu",
  "browser.tab.picked",
  "browser.error",
  "pong",
  "error",
]);

/**
 * A tool call's status in the SDK's one spelling. The host writes ACP's
 * `in_progress` as `inprogress` (the enum's Debug name, lowercased); some
 * agents' transcripts already say `in_progress`. Both decode the same.
 */
export function normalizeToolStatus(status: string | null | undefined): ToolCallStatus | null {
  if (status === undefined || status === null) return null;
  return status === "inprogress" ? "in_progress" : status;
}

function normalizeEvent(event: SessionEvent): void {
  if (event.type === "agent.tool_call" || event.type === "agent.tool_call_update") {
    event.payload.status = normalizeToolStatus(event.payload.status);
  }
}

/** Normalize, in place, the tool statuses a freshly parsed frame carries. */
function normalizeFrame(frame: ServerFrame): ServerFrame {
  switch (frame.type) {
    case "snapshot":
      for (const tc of frame.snapshot.tool_calls ?? []) tc.status = normalizeToolStatus(tc.status);
      break;
    case "replay":
      for (const ev of frame.events ?? []) normalizeEvent(ev);
      break;
    case "event":
      normalizeEvent(frame.event);
      break;
  }
  return frame;
}

function normalizeHistory(turns: HistoryTurn[]): void {
  for (const turn of turns) {
    for (const b of turn.blocks) if (b.type === "tool_call") b.status = normalizeToolStatus(b.status);
  }
}

/** Normalize, in place, an operation's freshly decoded result. */
export function decodeResult<O extends OpName>(op: O, result: OpResult<O>): OpResult<O> {
  if (op === "session.history") normalizeHistory((result as OpResult<"session.history">).turns ?? []);
  return result;
}

/** Parse one event-socket text frame; `null` for anything that is not V1. */
export function decodeServerFrame(text: string): ServerFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const type = (value as { type?: unknown }).type;
  if (typeof type !== "string" || !FRAME_TYPES.has(type)) return null;
  return normalizeFrame(value as ServerFrame);
}

/** Unpadded base64url of the UTF-8 bytes of `text`. */
export function base64Url(text: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
