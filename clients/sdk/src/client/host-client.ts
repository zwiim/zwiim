// HostClient: one connection to one host. Owns the connection lifecycle
// (hello → event socket → live, heartbeat, reconnect with backoff), the host
// state feed, session subscriptions with replay cursors, terminal stream
// sync, and typed requests. It holds no authoritative state — only cursors
// needed to resume.

import {
  applyTerminalEvent,
  applyTerminalSnapshot,
  desyncTerminal,
  initialTerminalCursor,
  type TerminalCursor,
  type TerminalOutput,
} from "../state/terminal";
import type { SessionFrame } from "../state/session";
import { decodeResult } from "../transport/codec";
import type { EventChannel, Transport } from "../transport/transport";
import {
  MUTATING_OPS,
  type HostFeedEvent,
  type HostHello,
  type HostSnapshot,
  type OpName,
  type OpParams,
  type OpResult,
  type ServerFrame,
  type SessionEvent,
} from "../types/protocol";
import {
  backoffDelay,
  DEFAULT_BACKOFF,
  type BackoffOptions,
  type ConnectionState,
  type ConnectionStatus,
} from "./connection";
import { evaluateCompatibility, helloClient, type ClientInfo, type Compatibility } from "./compat";
import { isClientError, ZwiimClientError } from "./errors";

export type TransportSource = Transport | (() => Transport | Promise<Transport>);

/**
 * Failures retrying cannot fix, and the status each leaves the client in:
 * the caller must re-authenticate, re-pair or pick another host, then
 * `start()` again. Everything else reconnects with backoff.
 */
const TERMINAL: Readonly<Record<string, "unauthenticated" | "error">> = {
  unauthenticated: "unauthenticated",
  device_refused: "unauthenticated",
  host_mismatch: "error",
  host_unknown: "error",
  handshake_failed: "error",
  host_identity_mismatch: "error",
};

/**
 * Operations a client may still run while `incompatible`: enough to show the
 * Host's versions and diagnostics, and to ask an old Host to update itself.
 */
const ALLOWED_WHEN_INCOMPATIBLE: ReadonlySet<string> = new Set([
  "host.hello",
  "host.update.status",
  "host.update.check",
  "host.update.apply",
  "host.diagnostics",
]);

/** How often an incompatible Host is asked again (it may have been updated). */
const DEFAULT_INCOMPATIBLE_RETRY_MS = 60_000;

export interface HostClientOptions {
  /**
   * The transport, or a factory called before every connection attempt (so
   * a restarted host is rediscovered at its new URL/token).
   */
  transport: TransportSource;
  /** Refuse a host whose `host_id` differs (from discovery). */
  expectedHostId?: string;
  /**
   * This app, introduced in `host.hello`. Without it the Host gives no
   * verdict of its own; the SDK still decides compatibility.
   */
  client?: ClientInfo;
  /** Delay between compatibility re-checks while `incompatible`. */
  incompatibleRetryMs?: number;
  pingIntervalMs?: number;
  pongTimeoutMs?: number;
  backoff?: BackoffOptions;
  random?: () => number;
}

/** One step of the host state feed: a fresh baseline, or the next change. */
export type HostFeedUpdate =
  | { type: "snapshot"; snapshot: HostSnapshot }
  | { type: "event"; event: HostFeedEvent };

export interface HostClientListener {
  connection?(state: ConnectionState): void;
  hello?(hello: HostHello): void;
  feed?(update: HostFeedUpdate): void;
  session?(session_id: string, frame: SessionFrame): void;
  terminal?(terminal_id: string, output: TerminalOutput): void;
}

interface SessionSub {
  subscription_id: string;
  session_id: string;
  epoch: string | null;
  seq: number | null;
  state: "attaching" | "live" | "detached" | "error";
}

interface TerminalSub {
  cursor: TerminalCursor;
  /** Bumped per snapshot request; stale responses are dropped. */
  generation: number;
}

export class HostClient {
  private readonly source: TransportSource;
  private readonly expectedHostId: string | undefined;
  private readonly client: ClientInfo | undefined;
  private readonly incompatibleRetryMs: number;
  private readonly pingIntervalMs: number;
  private readonly pongTimeoutMs: number;
  private readonly backoff: BackoffOptions;
  private readonly random: () => number;

  private transport: Transport | null = null;
  private channel: EventChannel | null = null;
  private stateValue: ConnectionState = {
    status: "disconnected",
    compat: null,
    attempt: 0,
    hostId: null,
    error: null,
    retryInMs: null,
  };
  /** Bumped by start/stop; async steps of an older run are ignored. */
  private run = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private pongTimer: ReturnType<typeof setTimeout> | null = null;

  private nextSubId = 0;
  private readonly sessions = new Map<string, SessionSub>(); // by session_id
  private readonly subIndex = new Map<string, string>(); // subscription_id → session_id
  private readonly terminals = new Map<string, TerminalSub>();
  /** Host feed cursor; `epoch` is null while a snapshot is awaited. */
  private feed: { epoch: string | null; seq: number } = { epoch: null, seq: 0 };
  private readonly listeners = new Set<HostClientListener>();

  constructor(options: HostClientOptions) {
    this.source = options.transport;
    this.expectedHostId = options.expectedHostId;
    this.client = options.client;
    this.incompatibleRetryMs = options.incompatibleRetryMs ?? DEFAULT_INCOMPATIBLE_RETRY_MS;
    this.pingIntervalMs = options.pingIntervalMs ?? 20_000;
    this.pongTimeoutMs = options.pongTimeoutMs ?? 10_000;
    this.backoff = options.backoff ?? DEFAULT_BACKOFF;
    this.random = options.random ?? Math.random;
  }

  get state(): ConnectionState {
    return this.stateValue;
  }

  subscribe(listener: HostClientListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  // ─── lifecycle ─────────────────────────────────────────────────────────

  /** Connect (or retry after `unauthenticated`/`error`). No-op while active. */
  start(): void {
    const s = this.stateValue.status;
    if (s !== "disconnected" && s !== "unauthenticated" && s !== "error" && s !== "incompatible") return;
    this.run++;
    this.stateValue = { ...this.stateValue, attempt: 0, error: null };
    void this.attempt("connecting");
  }

  /** Close the event socket. The host and its sessions are unaffected. */
  stop(): void {
    this.run++;
    this.teardown();
    this.transport?.close?.();
    this.setState({ status: "disconnected", retryInMs: null });
  }

  /** Skip the remaining backoff delay. */
  retryNow(): void {
    const s = this.stateValue.status;
    if ((s !== "reconnecting" && s !== "incompatible") || this.retryTimer === null) return;
    clearTimeout(this.retryTimer);
    this.retryTimer = null;
    void this.attempt("reconnecting");
  }

  private async attempt(phase: "connecting" | "reconnecting"): Promise<void> {
    const run = this.run;
    this.setState({ status: phase, retryInMs: null });
    let hello: HostHello;
    try {
      const previous = this.transport;
      this.transport = typeof this.source === "function" ? await this.source() : this.source;
      if (previous && previous !== this.transport) previous.close?.();
      if (run !== this.run) return;
      hello = (await this.transport.request(
        "host.hello",
        this.client ? { client: helloClient(this.client) } : {},
      )) as HostHello;
    } catch (e) {
      if (run === this.run) this.fail(e);
      return;
    }
    if (run !== this.run) return;
    const bound = this.expectedHostId ?? this.stateValue.hostId;
    if (bound && hello.host_id !== bound) {
      this.fail(new ZwiimClientError("host_mismatch", "A different host is running at this address"));
      return;
    }
    const compat = evaluateCompatibility(hello);
    if (compat.status === "update_required") {
      this.emit((l) => l.hello?.(hello));
      this.becomeIncompatible(compat, hello.host_id);
      return;
    }
    this.setState({ status: "handshaking", hostId: hello.host_id, compat });
    this.emit((l) => l.hello?.(hello));
    this.openChannel(run);
  }

  /**
   * One side needs an update. Hold no event socket, refuse operations, and
   * ask again on a slow timer: an updated Host clears this by itself.
   */
  private becomeIncompatible(compat: Compatibility, hostId: string): void {
    this.teardown();
    const message =
      compat.status === "update_required" && compat.update === "host"
        ? "This Host needs an update"
        : "Update Zwiim to use this Host";
    this.setState({
      status: "incompatible",
      hostId,
      compat,
      attempt: 0,
      error: { code: "update_required", message },
      retryInMs: this.incompatibleRetryMs,
    });
    const run = this.run;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      if (run === this.run) void this.attempt("reconnecting");
    }, this.incompatibleRetryMs);
  }

  private openChannel(run: number): void {
    const channel = this.transport!.openEvents({
      onOpen: () => {
        if (channel !== this.channel || run !== this.run) return;
        this.onLive();
      },
      onFrame: (frame) => {
        if (channel !== this.channel || run !== this.run) return;
        this.onFrame(frame);
      },
      onClose: () => {
        if (channel !== this.channel || run !== this.run) return;
        this.channel = null;
        this.fail(new ZwiimClientError("network", "Event connection closed"));
      },
    });
    this.channel = channel;
  }

  private onLive(): void {
    this.setState({ status: "live", attempt: 0, error: null, retryInMs: null });
    this.startHeartbeat();
    this.subscribeHost();
    for (const sub of this.sessions.values()) this.sendSubscribe(sub);
    for (const id of this.terminals.keys()) {
      this.channel?.send({ op: "terminal.subscribe", terminal_id: id });
      void this.snapshotTerminal(id);
    }
  }

  private fail(e: unknown): void {
    const code = isClientError(e) ? e.code : "network";
    const message = e instanceof Error ? e.message : String(e);
    this.teardown();
    const terminal = TERMINAL[code];
    if (terminal) {
      this.setState({ status: terminal, error: { code, message }, retryInMs: null });
      return;
    }
    const attempt = this.stateValue.attempt;
    const delay = backoffDelay(attempt, this.random, this.backoff);
    this.setState({
      status: "reconnecting",
      attempt: attempt + 1,
      error: { code, message },
      retryInMs: delay,
    });
    const run = this.run;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      if (run === this.run) void this.attempt("reconnecting");
    }, delay);
  }

  private teardown(): void {
    this.stopHeartbeat();
    if (this.retryTimer !== null) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    const channel = this.channel;
    this.channel = null;
    channel?.close();
    this.feed = { epoch: null, seq: 0 };
    for (const sub of this.sessions.values()) {
      if (sub.state === "live") sub.state = "attaching";
    }
    for (const t of this.terminals.values()) {
      t.cursor = desyncTerminal(t.cursor);
      t.generation++;
    }
  }

  // ─── heartbeat ─────────────────────────────────────────────────────────

  private startHeartbeat(): void {
    this.stopHeartbeat();
    this.pingTimer = setInterval(() => {
      if (!this.channel) return;
      this.channel.send({ op: "ping" });
      if (this.pongTimer === null) {
        this.pongTimer = setTimeout(() => {
          this.pongTimer = null;
          // Silent socket: drop it and reconnect.
          const channel = this.channel;
          this.channel = null;
          channel?.close();
          this.fail(new ZwiimClientError("timeout", "Host stopped responding"));
        }, this.pongTimeoutMs);
      }
    }, this.pingIntervalMs);
  }

  private stopHeartbeat(): void {
    if (this.pingTimer !== null) clearInterval(this.pingTimer);
    if (this.pongTimer !== null) clearTimeout(this.pongTimer);
    this.pingTimer = null;
    this.pongTimer = null;
  }

  // ─── requests ──────────────────────────────────────────────────────────

  /**
   * Run one operation. Mutating operations are sent at most once: if the
   * connection drops mid-request the error is `unknown_outcome` and the
   * caller should refresh host state rather than resend.
   *
   * `opts.timeoutMs` overrides the transport's default, for calls known to
   * run long (publishing a new GitHub repository on add).
   */
  async request<O extends OpName>(
    op: O,
    params: OpParams<O>,
    opts?: { timeoutMs?: number },
  ): Promise<OpResult<O>> {
    if (this.stateValue.status === "incompatible" && !ALLOWED_WHEN_INCOMPATIBLE.has(op)) {
      throw new ZwiimClientError(
        "update_required",
        this.stateValue.error?.message ?? "An update is required to use this Host",
      );
    }
    const transport =
      this.transport ?? (typeof this.source === "function" ? await this.source() : this.source);
    this.transport = transport;
    try {
      const result = (await transport.request(op, params, {
        mutating: MUTATING_OPS.has(op),
        ...(opts?.timeoutMs !== undefined ? { timeoutMs: opts.timeoutMs } : {}),
      })) as OpResult<O>;
      return decodeResult(op, result);
    } catch (e) {
      const terminal = isClientError(e) ? TERMINAL[e.code] : undefined;
      if (terminal && isClientError(e)) {
        this.run++;
        this.teardown();
        this.setState({
          status: terminal,
          error: { code: e.code, message: e.message },
          retryInMs: null,
        });
      }
      throw e;
    }
  }

  // ─── sessions ──────────────────────────────────────────────────────────

  /** Stream a session's live state. Idempotent per session. */
  attachSession(session_id: string): void {
    let sub = this.sessions.get(session_id);
    if (!sub) {
      sub = {
        subscription_id: `sub-${++this.nextSubId}`,
        session_id,
        epoch: null,
        seq: null,
        state: "attaching",
      };
      this.sessions.set(session_id, sub);
      this.subIndex.set(sub.subscription_id, session_id);
    } else if (sub.state === "live" || sub.state === "attaching") {
      return;
    }
    sub.state = "attaching";
    this.sendSubscribe(sub);
  }

  detachSession(session_id: string): void {
    const sub = this.sessions.get(session_id);
    if (!sub) return;
    this.sessions.delete(session_id);
    this.subIndex.delete(sub.subscription_id);
    this.channel?.send({ op: "session.detach", subscription_id: sub.subscription_id });
  }

  /** Send a prompt; re-attaches if the session's agent had gone away. Never retried. */
  async sendMessage(params: OpParams<"session.send">): Promise<void> {
    await this.request("session.send", params);
    const sub = this.sessions.get(params.session_id);
    if (sub && (sub.state === "detached" || sub.state === "error")) {
      this.attachSession(params.session_id);
    }
  }

  /**
   * Send a follow-up: now when the agent is idle and nothing is queued, else
   * queued behind what is. One that went out re-attaches like `sendMessage`.
   * Never retried.
   */
  async sendFollowUp(params: OpParams<"session.follow_up">): Promise<OpResult<"session.follow_up">> {
    const result = await this.request("session.follow_up", params);
    if (result.outcome === "sent") this.reattachIfGone(params.session_id);
    return result;
  }

  /** Send a queued follow-up now, or move it first while a turn runs. Never retried. */
  async sendQueuedFollowUp(params: OpParams<"session.follow_up_send">): Promise<OpResult<"session.follow_up_send">> {
    const result = await this.request("session.follow_up_send", params);
    if (result.outcome === "sent") this.reattachIfGone(params.session_id);
    return result;
  }

  private reattachIfGone(session_id: string): void {
    const sub = this.sessions.get(session_id);
    if (sub && (sub.state === "detached" || sub.state === "error")) {
      this.attachSession(session_id);
    }
  }

  /**
   * Send the turn that could not reach the agent's provider again. The Host
   * starts the agent if it is gone, so a detached subscription re-attaches to
   * see the turn; without that its events never reach the view. Never retried.
   */
  async retrySession(params: OpParams<"session.retry">): Promise<void> {
    await this.request("session.retry", params);
    const sub = this.sessions.get(params.session_id);
    if (sub && (sub.state === "detached" || sub.state === "error")) {
      this.attachSession(params.session_id);
    }
  }

  /**
   * Take a session back to before a turn. A chat rewind resumes the agent of
   * a session that was not live, so a detached subscription re-attaches to
   * see the rewound transcript; after a files-only reset the agent stays
   * down and a subscribe would only be refused again. Never retried.
   */
  async revertSession(params: OpParams<"session.revert">): Promise<OpResult<"session.revert">> {
    const result = await this.request("session.revert", params);
    const sub = this.sessions.get(params.session_id);
    if (result.chat_rewound && sub && (sub.state === "detached" || sub.state === "error")) {
      this.attachSession(params.session_id);
    }
    return result;
  }

  private sendSubscribe(sub: SessionSub): void {
    if (this.stateValue.status !== "live" || !this.channel) return;
    this.emit((l) => l.session?.(sub.session_id, { type: "attaching" }));
    this.channel.send({
      op: "session.subscribe",
      subscription_id: sub.subscription_id,
      session_id: sub.session_id,
      ...(sub.epoch !== null && sub.seq !== null ? { epoch: sub.epoch, since_seq: sub.seq } : {}),
    });
  }

  // ─── host state feed ───────────────────────────────────────────────────

  /** Ask for a fresh snapshot; events are ignored until it arrives. */
  private subscribeHost(): void {
    if (this.stateValue.status !== "live" || !this.channel) return;
    this.feed = { epoch: null, seq: 0 };
    this.channel.send({ op: "host.subscribe" });
  }

  private onFeedEvent(epoch: string, event: HostFeedEvent): void {
    const cursor = this.feed;
    if (cursor.epoch === null) return;
    if (epoch !== cursor.epoch) {
      // The host restarted under us: its old baseline no longer holds.
      this.subscribeHost();
      return;
    }
    if (event.seq <= cursor.seq) return;
    if (event.seq !== cursor.seq + 1) {
      // Missed changes: no replay in V1, start over from a snapshot.
      this.subscribeHost();
      return;
    }
    cursor.seq = event.seq;
    this.emit((l) => l.feed?.({ type: "event", event }));
  }

  private onFrame(frame: ServerFrame): void {
    switch (frame.type) {
      case "host.snapshot": {
        const { type: _, ...snapshot } = frame;
        this.feed = { epoch: snapshot.epoch, seq: snapshot.seq };
        this.emit((l) => l.feed?.({ type: "snapshot", snapshot }));
        return;
      }
      case "host.event":
        this.onFeedEvent(frame.epoch, frame.event);
        return;
      case "pong":
        if (this.pongTimer !== null) clearTimeout(this.pongTimer);
        this.pongTimer = null;
        return;
      case "terminal":
        this.onTerminalEvent(frame.event.terminal_id, frame.event);
        return;
      case "error": {
        if (frame.subscription_id === null) {
          // Only the host feed answers without a subscription: it could not
          // start, so reconnect and retry rather than run without state.
          this.fail(new ZwiimClientError(frame.error.code, frame.error.message));
          return;
        }
        const sub = frame.subscription_id ? this.subFor(frame.subscription_id) : undefined;
        if (!sub) return;
        sub.state = "error";
        this.emit((l) => l.session?.(sub.session_id, { type: "error", error: frame.error }));
        return;
      }
      case "detached": {
        const sub = this.subFor(frame.subscription_id);
        if (!sub) return;
        if (frame.reason === "lagged") {
          // Events were lost; resume from the cursor (replay or snapshot).
          sub.state = "attaching";
          this.sendSubscribe(sub);
          return;
        }
        if (frame.reason === "server_shutdown") return; // reconnect resubscribes
        sub.state = "detached";
        sub.epoch = null;
        sub.seq = null;
        this.emit((l) => l.session?.(sub.session_id, { type: "detached", reason: frame.reason }));
        return;
      }
      case "snapshot": {
        const sub = this.subFor(frame.subscription_id);
        if (!sub) return;
        sub.state = "live";
        sub.epoch = frame.epoch;
        sub.seq = frame.event_seq;
        this.emit((l) =>
          l.session?.(sub.session_id, {
            type: "snapshot",
            epoch: frame.epoch,
            seq: frame.event_seq,
            snapshot: frame.snapshot,
          }),
        );
        return;
      }
      case "replay": {
        const sub = this.subFor(frame.subscription_id);
        if (!sub) return;
        if (sub.epoch !== null && frame.epoch !== sub.epoch) {
          // Not our cursor's connection: start over from a snapshot.
          sub.epoch = null;
          sub.seq = null;
          sub.state = "attaching";
          this.sendSubscribe(sub);
          return;
        }
        const after = sub.seq ?? 0;
        const events = frame.events.filter((e) => e.seq > after);
        sub.state = "live";
        sub.epoch = frame.epoch;
        sub.seq = Math.max(after, frame.high_water_seq);
        this.emit((l) =>
          l.session?.(sub.session_id, {
            type: "replay",
            epoch: frame.epoch,
            events,
            high_water_seq: frame.high_water_seq,
          }),
        );
        return;
      }
      case "event": {
        const sub = this.subFor(frame.subscription_id);
        if (!sub || sub.epoch === null) return;
        const event: SessionEvent = frame.event;
        if (sub.seq !== null && event.seq <= sub.seq) return;
        sub.seq = event.seq;
        this.emit((l) => l.session?.(sub.session_id, { type: "event", event }));
        return;
      }
    }
  }

  private subFor(subscription_id: string): SessionSub | undefined {
    const session_id = this.subIndex.get(subscription_id);
    return session_id === undefined ? undefined : this.sessions.get(session_id);
  }

  // ─── terminals ─────────────────────────────────────────────────────────

  /** Stream a terminal: subscribe, then snapshot, then contiguous output. */
  attachTerminal(terminal_id: string): void {
    if (this.terminals.has(terminal_id)) {
      void this.snapshotTerminal(terminal_id);
      return;
    }
    this.terminals.set(terminal_id, { cursor: initialTerminalCursor(), generation: 0 });
    if (this.stateValue.status === "live" && this.channel) {
      this.channel.send({ op: "terminal.subscribe", terminal_id });
      void this.snapshotTerminal(terminal_id);
    }
  }

  detachTerminal(terminal_id: string): void {
    if (!this.terminals.delete(terminal_id)) return;
    this.channel?.send({ op: "terminal.unsubscribe", terminal_id });
  }

  private async snapshotTerminal(terminal_id: string): Promise<void> {
    const t = this.terminals.get(terminal_id);
    if (!t) return;
    const generation = ++t.generation;
    t.cursor = desyncTerminal(t.cursor);
    let snap;
    try {
      snap = await this.request("terminal.snapshot", { terminal_id });
    } catch {
      return; // resynced on reconnect
    }
    const current = this.terminals.get(terminal_id);
    if (current !== t || t.generation !== generation) return;
    const step = applyTerminalSnapshot(t.cursor, snap);
    t.cursor = step.cursor;
    this.emitTerminal(terminal_id, step.outputs);
  }

  private onTerminalEvent(terminal_id: string, event: Parameters<typeof applyTerminalEvent>[1]) {
    const t = this.terminals.get(terminal_id);
    if (!t) return;
    const step = applyTerminalEvent(t.cursor, event);
    t.cursor = step.cursor;
    this.emitTerminal(terminal_id, step.outputs);
  }

  private emitTerminal(terminal_id: string, outputs: TerminalOutput[]): void {
    for (const out of outputs) {
      if (out.kind === "resync") {
        void this.snapshotTerminal(terminal_id);
        continue;
      }
      this.emit((l) => l.terminal?.(terminal_id, out));
    }
  }

  // ─── plumbing ──────────────────────────────────────────────────────────

  private setState(patch: Partial<ConnectionState> & { status?: ConnectionStatus }): void {
    this.stateValue = { ...this.stateValue, ...patch };
    const state = this.stateValue;
    this.emit((l) => l.connection?.(state));
  }

  private emit(fn: (l: HostClientListener) => void): void {
    for (const l of [...this.listeners]) fn(l);
  }
}
