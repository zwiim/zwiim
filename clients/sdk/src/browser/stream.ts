// A browser view's stream socket (design 7.2): the tab's frames as binary
// messages, the viewer's input as JSON text, and the Host's answer to input
// it could not deliver as an `error` text message.

import type { BrowserInput, WireError } from "../types/protocol";
import { decodeBrowserFrame, type BrowserScreenFrame } from "./frame";

/** Subprotocol of `/zwiim/v1/browser/stream/<stream_id>`. */
export const BROWSER_STREAM_SUBPROTOCOL = "zwiim-browser-stream";

export interface BrowserStreamHandlers {
  onOpen?(): void;
  onFrame(frame: BrowserScreenFrame): void;
  /** The Host could not deliver an input message. The stream stays open. */
  onInputError?(error: WireError): void;
  /**
   * The stream ended: closed by either side, its tab closed, the engine
   * stopped, or frames came in a newer format (`unsupported_version`). Open
   * a new view to watch again.
   */
  onClose(info: { code: number; reason: string }): void;
}

export interface BrowserStream {
  /** Dropped until the socket is open. */
  send(input: BrowserInput): void;
  close(): void;
}

/** What a transport that can carry browser streams offers. */
export interface BrowserStreamTransport {
  openBrowserStream(stream_id: string, handlers: BrowserStreamHandlers): BrowserStream;
}

type Socket = Pick<WebSocket, "readyState" | "send" | "close"> & {
  binaryType: string;
  onopen: ((ev: Event) => void) | null;
  onmessage: ((ev: MessageEvent) => void) | null;
  onerror: ((ev: Event) => void) | null;
  onclose: ((ev: CloseEvent) => void) | null;
};

/** Run `handlers` on an opened stream socket. */
export function driveBrowserStream(socket: Socket, handlers: BrowserStreamHandlers): BrowserStream {
  socket.binaryType = "arraybuffer";
  let closed = false;
  const finish = (code: number, reason: string) => {
    if (closed) return;
    closed = true;
    handlers.onClose({ code, reason });
  };
  // Reported before the socket closes, so this reason wins over the close event's.
  const close = (code: number, reason: string) => {
    finish(code, reason);
    socket.close();
  };
  socket.onopen = () => handlers.onOpen?.();
  socket.onmessage = (ev: MessageEvent) => {
    if (closed) return;
    if (typeof ev.data === "string") {
      const error = inputError(ev.data);
      if (error) handlers.onInputError?.(error);
      return;
    }
    if (!(ev.data instanceof ArrayBuffer)) return;
    const result = decodeBrowserFrame(ev.data);
    if (result.kind === "frame") handlers.onFrame(result.frame);
    else if (result.kind === "unsupported_version") close(1000, "unsupported_version");
  };
  socket.onerror = () => finish(1006, "error");
  socket.onclose = (ev: CloseEvent) => finish(ev.code, ev.reason);
  return {
    send(input) {
      if (!closed && socket.readyState === 1) socket.send(JSON.stringify(input));
    },
    close() {
      close(1000, "closed by client");
    },
  };
}

function inputError(text: string): WireError | null {
  try {
    const value = JSON.parse(text) as { type?: unknown; error?: { code?: unknown; message?: unknown } };
    if (value?.type !== "error" || typeof value.error?.code !== "string") return null;
    return { code: value.error.code, message: String(value.error.message ?? "") };
  } catch {
    return null;
  }
}
