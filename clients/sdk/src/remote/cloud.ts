// The device side of Zwiim Cloud: account-authenticated host discovery,
// connection tickets, and the relay socket. This is connectivity only —
// nothing here carries Zwiim protocol content. The relay socket moves the
// bytes of the Noise tunnel (`./tunnel`), which the Cloud cannot read.
//
//   GET  /v1/hosts                  the account's hosts, with presence
//   GET  /v1/usage                  the account's plan and relay usage
//   POST /v1/hosts/{id}/tickets     a single-use ticket to reach one
//   POST /v1/enrollments            a single-use host enrollment token
//   WS   /v1/relay/device           {"type":"device","ticket"} → {"type":"ready"} → binary

import { ZwiimClientError } from "../client/errors";
import type { ByteChannel } from "./tunnel";

type WebSocketCtor = new (url: string, protocols?: string | string[]) => WebSocket;

/** One of the account's hosts, as the Cloud knows it. */
export interface CloudHost {
  host_id: string;
  name: string | null;
  /** X25519 key the host enrolled with. Metadata: devices pin the key from pairing. */
  static_pubkey: string;
  online: boolean;
  /** Unix seconds of the last heartbeat since the Cloud started. */
  last_seen_at: number | null;
}

export interface CloudTicket {
  ticket: string;
  /** Unix seconds. */
  expires_at: number;
}

/**
 * The kind of traffic a relay connection carries; the Cloud counts and
 * limits each separately. `browser` is for Agent View and Live App only.
 */
export type RelayChannel = "control" | "browser";

/** Bytes relayed this period and the plan's limit; `null` is unlimited. */
export interface CloudChannelUsage {
  used: number;
  limit: number | null;
}

/**
 * The account's plan and relay usage this period. `plan` is `null` when the
 * Cloud keeps no plan for the account: nothing is limited and the period
 * fields are absent.
 */
export interface CloudUsage {
  plan: { id: string; name: string } | null;
  /** Unix seconds. */
  period_start?: number;
  /** Unix seconds. */
  resets_at?: number;
  channels?: { browser: CloudChannelUsage; total: CloudChannelUsage };
}

/**
 * The account reached a relay limit of its plan: a refused ticket (`429`),
 * with which limit and when it lifts. A relay closed at a limit (4429)
 * carries no details; `CloudClient.usage()` has them.
 */
export class RelayQuotaError extends ZwiimClientError {
  /** `browser`: the browser limit; `total`: every channel together, which stops all relays. */
  readonly limit: "browser" | "total";
  /** The plan id. */
  readonly plan: string;
  readonly usedBytes: number;
  readonly limitBytes: number;
  /** Unix seconds. */
  readonly resetsAt: number;

  constructor(d: { channel: "browser" | "total"; plan: string; used_bytes: number; limit_bytes: number; resets_at: number }) {
    const what = d.channel === "browser" ? "remote browser" : "relay";
    super("relay_quota_exceeded", `The account reached its ${what} limit for this period`, 429, { ...d });
    this.name = "RelayQuotaError";
    this.limit = d.channel;
    this.plan = d.plan;
    this.usedBytes = d.used_bytes;
    this.limitBytes = d.limit_bytes;
    this.resetsAt = d.resets_at;
  }
}

export interface EnrollmentToken {
  enrollment_token: string;
  /** Unix seconds. */
  expires_at: number;
}

export interface CloudClientOptions {
  /** The Cloud origin: `https://…`, or `http://` on loopback only. */
  url: string;
  /** The account's access token, or a function returning a current one. */
  accessToken: string | (() => string | Promise<string>);
  fetch?: typeof fetch;
  WebSocket?: WebSocketCtor;
  /** HTTP request timeout. */
  timeoutMs?: number;
  /** How long a relay socket may take to reach the host. */
  dialTimeoutMs?: number;
}

/** WebSocket close codes the Cloud uses (`zwiim_cloud::proto`). */
export const CLOUD_CLOSE = {
  protocol: 4400,
  unauthenticated: 4401,
  quotaExceeded: 4429,
  hostOffline: 4503,
  restarting: 1012,
} as const;

const DEFAULT_TIMEOUT_MS = 15_000;
const DEFAULT_DIAL_TIMEOUT_MS = 20_000;

function isLoopback(hostname: string): boolean {
  const h = hostname.toLowerCase();
  return h === "localhost" || h === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(h);
}

/**
 * The canonical origin of a Cloud URL, as the host's `normalize_cloud_url`:
 * https (http on loopback only), no credentials, path, query or fragment.
 */
export function normalizeCloudUrl(raw: string): string {
  const text = raw.trim();
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    throw new ZwiimClientError("invalid_input", "Not a valid Cloud URL");
  }
  if (
    url.username ||
    url.password ||
    text.includes("?") ||
    text.includes("#") ||
    url.pathname !== "/" ||
    !url.hostname
  ) {
    throw new ZwiimClientError("invalid_input", "Not a valid Cloud URL");
  }
  if (url.protocol === "http:" && !isLoopback(url.hostname)) {
    throw new ZwiimClientError("invalid_input", "The Cloud URL must use https");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new ZwiimClientError("invalid_input", "Not a valid Cloud URL");
  }
  return url.origin;
}

/** A host id the Cloud accepts (`valid_host_id`). */
export function isValidHostId(hostId: string): boolean {
  return /^[A-Za-z0-9_-]{1,128}$/.test(hostId);
}

/**
 * What an early relay close means: the host is offline (4503), the Cloud is
 * restarting or gone (1012, 1006), the ticket was refused (4401: retry with
 * a fresh one), the account reached a relay limit (4429), or the host ended
 * the stream (1000).
 */
export function relayCloseError({ code, reason }: { code: number; reason: string }): ZwiimClientError {
  const detail = `${code}${reason ? ` ${reason}` : ""}`;
  if (code === CLOUD_CLOSE.hostOffline) {
    return new ZwiimClientError("host_offline", "The host is offline");
  }
  if (code === 1000) {
    return new ZwiimClientError("network", "The host closed the tunnel");
  }
  if (code === CLOUD_CLOSE.unauthenticated) {
    return new ZwiimClientError("network", "The Cloud refused the connection ticket");
  }
  if (code === CLOUD_CLOSE.quotaExceeded) {
    return new ZwiimClientError("relay_quota_exceeded", "The account reached a relay limit of its plan");
  }
  return new ZwiimClientError("cloud_unavailable", `Relay connection closed (${detail})`);
}

/** Account-scoped access to one Zwiim Cloud. */
export class CloudClient {
  readonly origin: string;
  private readonly accessToken: CloudClientOptions["accessToken"];
  private readonly fetchImpl: typeof fetch;
  private readonly WebSocketImpl: WebSocketCtor;
  private readonly timeoutMs: number;
  private readonly dialTimeoutMs: number;

  constructor(options: CloudClientOptions) {
    this.origin = normalizeCloudUrl(options.url);
    this.accessToken = options.accessToken;
    this.fetchImpl = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.WebSocketImpl = options.WebSocket ?? globalThis.WebSocket;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.dialTimeoutMs = options.dialTimeoutMs ?? DEFAULT_DIAL_TIMEOUT_MS;
  }

  /** The account's hosts and whether each is online. */
  async hosts(): Promise<CloudHost[]> {
    const body = await this.call("GET", "/v1/hosts");
    if (!Array.isArray(body)) throw new ZwiimClientError("invalid_response", "Malformed host list");
    return body as CloudHost[];
  }

  /** The account's plan and relay usage this period. */
  async usage(): Promise<CloudUsage> {
    const body = (await this.call("GET", "/v1/usage")) as CloudUsage;
    if (typeof body !== "object" || body === null || !("plan" in body)) {
      throw new ZwiimClientError("invalid_response", "Malformed usage");
    }
    return body;
  }

  /**
   * A single-use ticket for one of the account's hosts, for a relay on
   * `channel`. `host_unknown` if the account has no such host;
   * `host_offline` if it is not connected; a `RelayQuotaError` at a limit
   * of the account's plan.
   */
  async ticket(hostId: string, channel: RelayChannel = "control"): Promise<CloudTicket> {
    if (!isValidHostId(hostId)) throw new ZwiimClientError("host_unknown", "Not a host id");
    // A control ticket sends no body, as before relay channels.
    const body = (await this.call(
      "POST",
      `/v1/hosts/${hostId}/tickets`,
      true,
      channel === "control" ? undefined : { channel },
    )) as CloudTicket;
    if (typeof body?.ticket !== "string") {
      throw new ZwiimClientError("invalid_response", "Malformed ticket");
    }
    return body;
  }

  /** A single-use token a host redeems (locally, `cloud.enroll`) to join the account. */
  async enrollmentToken(): Promise<EnrollmentToken> {
    const body = (await this.call("POST", "/v1/enrollments")) as EnrollmentToken;
    if (typeof body?.enrollment_token !== "string") {
      throw new ZwiimClientError("invalid_response", "Malformed enrollment token");
    }
    return body;
  }

  /**
   * Open a relay socket with `ticket` and wait until the host has joined.
   * Rejects as `relayCloseError` classifies the Cloud's close.
   */
  dial(ticket: string): Promise<ByteChannel> {
    const wsUrl = `${this.origin.replace(/^http/, "ws")}/v1/relay/device`;
    return new Promise((resolve, reject) => {
      let socket: WebSocket;
      try {
        socket = new this.WebSocketImpl(wsUrl);
      } catch {
        reject(new ZwiimClientError("cloud_unavailable", "Zwiim Cloud unreachable"));
        return;
      }
      socket.binaryType = "arraybuffer";
      let ready = false;
      let done = false;
      const timer = setTimeout(() => {
        if (ready || done) return;
        done = true;
        socket.close();
        reject(new ZwiimClientError("host_offline", "The host did not answer in time"));
      }, this.dialTimeoutMs);

      // After `ready`: the byte channel.
      const early: Uint8Array[] = [];
      let handlers: Parameters<ByteChannel["listen"]>[0] | null = null;
      let closedInfo: { code: number; reason: string } | null = null;
      const closeChannel = (info: { code: number; reason: string }) => {
        if (closedInfo) return;
        closedInfo = info;
        handlers?.onClose(info);
      };
      const channel: ByteChannel = {
        send(bytes) {
          if (!closedInfo && socket.readyState === 1) socket.send(bytes);
        },
        close() {
          try {
            socket.close(1000);
          } finally {
            closeChannel({ code: 1000, reason: "closed by device" });
          }
        },
        listen(h) {
          handlers = h;
          for (const bytes of early.splice(0)) h.onData(bytes);
          if (closedInfo) h.onClose(closedInfo);
        },
      };

      socket.onopen = () => socket.send(JSON.stringify({ type: "device", ticket }));
      socket.onmessage = (ev: MessageEvent) => {
        if (!ready) {
          let type: unknown;
          try {
            type = typeof ev.data === "string" ? JSON.parse(ev.data).type : undefined;
          } catch {
            type = undefined;
          }
          if (type !== "ready") {
            done = true;
            clearTimeout(timer);
            socket.close(CLOUD_CLOSE.protocol);
            reject(new ZwiimClientError("cloud_unavailable", "Malformed relay handshake"));
            return;
          }
          ready = true;
          done = true;
          clearTimeout(timer);
          resolve(channel);
          return;
        }
        if (typeof ev.data === "string") {
          // Only binary is relayed once open.
          socket.close(CLOUD_CLOSE.protocol);
          closeChannel({ code: CLOUD_CLOSE.protocol, reason: "malformed" });
          return;
        }
        const bytes = new Uint8Array(ev.data as ArrayBuffer);
        if (handlers) handlers.onData(bytes);
        else early.push(bytes);
      };
      socket.onerror = () => {
        if (ready) return closeChannel({ code: 1006, reason: "error" });
        if (done) return;
        done = true;
        clearTimeout(timer);
        reject(new ZwiimClientError("cloud_unavailable", "Zwiim Cloud unreachable"));
      };
      socket.onclose = (ev: CloseEvent) => {
        if (ready) return closeChannel({ code: ev.code, reason: ev.reason });
        if (done) return;
        done = true;
        clearTimeout(timer);
        reject(relayCloseError({ code: ev.code, reason: ev.reason }));
      };
    });
  }

  private async token(): Promise<string> {
    return typeof this.accessToken === "function" ? await this.accessToken() : this.accessToken;
  }

  private async call(method: "GET" | "POST", path: string, hostScoped = false, json?: unknown): Promise<unknown> {
    const token = await this.token();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      let res: Response;
      try {
        res = await this.fetchImpl(`${this.origin}${path}`, {
          method,
          headers: {
            authorization: `Bearer ${token}`,
            ...(json === undefined ? {} : { "content-type": "application/json" }),
          },
          ...(json === undefined ? {} : { body: JSON.stringify(json) }),
          signal: controller.signal,
        });
      } catch {
        throw new ZwiimClientError("cloud_unavailable", "Zwiim Cloud unreachable");
      }
      let body: unknown;
      try {
        body = JSON.parse(await res.text());
      } catch {
        body = null;
      }
      if (res.ok) return body;
      const code = (body as { error?: { code?: unknown } } | null)?.error?.code;
      if (res.status === 401) {
        throw new ZwiimClientError("unauthenticated", "Zwiim Cloud refused the account token", 401);
      }
      if (hostScoped && res.status === 404) {
        // Unknown, or another account's: the Cloud does not say which.
        throw new ZwiimClientError("host_unknown", "This account has no such host", 404);
      }
      if (code === "relay_quota_exceeded") {
        const quota = parseQuota((body as { error?: unknown }).error);
        if (quota) throw new RelayQuotaError(quota);
      }
      if (code === "host_offline") {
        throw new ZwiimClientError("host_offline", "The host is offline", res.status);
      }
      if (res.status >= 500 || res.status === 429) {
        throw new ZwiimClientError("cloud_unavailable", `Zwiim Cloud: HTTP ${res.status}`, res.status);
      }
      throw new ZwiimClientError(
        typeof code === "string" ? code : "invalid_response",
        `Zwiim Cloud: HTTP ${res.status}`,
        res.status,
      );
    } finally {
      clearTimeout(timer);
    }
  }
}

function parseQuota(raw: unknown): ConstructorParameters<typeof RelayQuotaError>[0] | null {
  const e = raw as Record<string, unknown> | null;
  if (!e || (e.channel !== "browser" && e.channel !== "total") || typeof e.plan !== "string") return null;
  const n = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);
  const [used, limit, resets] = [n(e.used_bytes), n(e.limit_bytes), n(e.resets_at)];
  if (used === null || limit === null || resets === null) return null;
  return { channel: e.channel, plan: e.plan, used_bytes: used, limit_bytes: limit, resets_at: resets };
}
