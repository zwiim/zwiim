import { ZwiimClientError } from "../client/errors";
import {
  EVENTS_SUBPROTOCOL,
  PROTOCOL_PREFIX,
  TOKEN_SUBPROTOCOL_PREFIX,
  type ClientMessage,
  type OpName,
} from "../types/protocol";
import { base64Url, decodeServerFrame } from "./codec";
import type {
  EventChannel,
  EventChannelHandlers,
  RequestOptions,
  Transport,
} from "./transport";

type WebSocketCtor = new (url: string, protocols?: string | string[]) => WebSocket;

export interface LocalTransportOptions {
  /** Host base URL from discovery, e.g. `http://127.0.0.1:3080`. */
  url: string;
  token: string;
  fetch?: typeof fetch;
  WebSocket?: WebSocketCtor;
  /** Default request timeout. */
  timeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 60_000;

/** Localhost HTTP + WebSocket transport to a host on the same machine. */
export class LocalTransport implements Transport {
  private readonly base: string;
  private readonly token: string;
  private readonly fetchImpl: typeof fetch;
  private readonly WebSocketImpl: WebSocketCtor;
  private readonly timeoutMs: number;

  constructor(options: LocalTransportOptions) {
    this.base = options.url.replace(/\/+$/, "");
    this.token = options.token;
    this.fetchImpl = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.WebSocketImpl = options.WebSocket ?? globalThis.WebSocket;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  }

  async request(op: OpName, params: unknown, options: RequestOptions = {}): Promise<unknown> {
    const mutating = options.mutating ?? false;
    const lost = (message: string) =>
      new ZwiimClientError(mutating ? "unknown_outcome" : "network", message);

    const controller = new AbortController();
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, options.timeoutMs ?? this.timeoutMs);

    try {
      let res: Response;
      try {
        res = await this.fetchImpl(`${this.base}${PROTOCOL_PREFIX}/${op}`, {
          method: "POST",
          headers: {
            authorization: `Bearer ${this.token}`,
            "content-type": "application/json",
          },
          body: JSON.stringify(params ?? {}),
          signal: controller.signal,
        });
      } catch {
        if (timedOut && !mutating) {
          throw new ZwiimClientError("timeout", `${op} timed out`);
        }
        throw lost(`${op}: host unreachable`);
      }

      let body: unknown;
      try {
        body = JSON.parse(await res.text());
      } catch {
        if (res.ok || mutating) throw lost(`${op}: response lost`);
        throw new ZwiimClientError("invalid_response", `${op}: HTTP ${res.status}`, res.status);
      }

      if (res.ok) return body;
      const error = (body as { error?: { code?: unknown; message?: unknown; details?: unknown } } | null)
        ?.error;
      if (error && typeof error.code === "string") {
        const details =
          error.details && typeof error.details === "object" && !Array.isArray(error.details)
            ? (error.details as Record<string, unknown>)
            : undefined;
        throw new ZwiimClientError(error.code, String(error.message ?? ""), res.status, details);
      }
      if (mutating) throw lost(`${op}: HTTP ${res.status}`);
      throw new ZwiimClientError("invalid_response", `${op}: HTTP ${res.status}`, res.status);
    } finally {
      clearTimeout(timer);
    }
  }

  openEvents(handlers: EventChannelHandlers): EventChannel {
    const wsUrl = `${this.base.replace(/^http/, "ws")}${PROTOCOL_PREFIX}/ws`;
    const socket = new this.WebSocketImpl(wsUrl, [
      EVENTS_SUBPROTOCOL,
      `${TOKEN_SUBPROTOCOL_PREFIX}${base64Url(this.token)}`,
    ]);
    let closed = false;
    const finish = (code: number, reason: string) => {
      if (closed) return;
      closed = true;
      handlers.onClose({ code, reason });
    };
    socket.onopen = () => handlers.onOpen();
    socket.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data !== "string") return;
      const frame = decodeServerFrame(ev.data);
      if (frame) handlers.onFrame(frame);
    };
    socket.onerror = () => finish(1006, "error");
    socket.onclose = (ev: CloseEvent) => finish(ev.code, ev.reason);
    return {
      send(msg: ClientMessage) {
        if (socket.readyState === 1) socket.send(JSON.stringify(msg));
      },
      close() {
        try {
          socket.close();
        } finally {
          finish(1000, "closed by client");
        }
      },
    };
  }
}
