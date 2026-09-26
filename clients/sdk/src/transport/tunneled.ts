import { ZwiimClientError } from "../client/errors";
import type { Tunnel } from "../remote/tunnel";
import type { ClientMessage, OpName } from "../types/protocol";
import { decodeServerFrame } from "./codec";
import type {
  EventChannel,
  EventChannelHandlers,
  RequestOptions,
  Transport,
} from "./transport";

/**
 * A host reached through the device tunnel: the same `/zwiim/v1` requests
 * and event socket as `LocalTransport`, carried in the end-to-end encrypted
 * tunnel. Subclasses only say how a tunnel is opened (through the Cloud
 * relay, or directly on the LAN); everything the tunnel carries is the same.
 *
 * The tunnel is opened on first use and shared by requests and event
 * streams. When it drops, in-flight requests fail (`unknown_outcome` if
 * mutating) and open event streams close; the next request opens a new
 * tunnel. Nothing is resent.
 */
export abstract class TunnelTransport implements Transport {
  private tunnel: Tunnel | null = null;
  private connecting: Promise<Tunnel> | null = null;
  /** Bumped by `close()`; a connect that finishes after it is discarded. */
  private generation = 0;

  protected constructor(protected readonly timeoutMs: number) {}

  /** Open a new tunnel to the host: dial, then the Noise handshake. */
  protected abstract openTunnel(): Promise<Tunnel>;

  /** Whether a tunnel is currently open. */
  get connected(): boolean {
    return this.tunnel?.isOpen ?? false;
  }

  /** Open the tunnel now, rather than on first use. */
  async connect(): Promise<void> {
    await this.ensureTunnel();
  }

  async request(op: OpName, params: unknown, options: RequestOptions = {}): Promise<unknown> {
    const mutating = options.mutating ?? false;
    // A failure here happens before anything is sent: never `unknown_outcome`.
    const tunnel = await this.ensureTunnel();
    const res = await tunnel.request(op, params ?? {}, {
      timeoutMs: options.timeoutMs ?? this.timeoutMs,
      mutating,
    });
    if (res.status >= 200 && res.status < 300) return res.body;
    const error = (res.body as { error?: { code?: unknown; message?: unknown; details?: unknown } } | null)
      ?.error;
    if (error && typeof error.code === "string") {
      const details =
        error.details && typeof error.details === "object" && !Array.isArray(error.details)
          ? (error.details as Record<string, unknown>)
          : undefined;
      throw new ZwiimClientError(error.code, String(error.message ?? ""), res.status, details);
    }
    if (mutating) throw new ZwiimClientError("unknown_outcome", `${op}: HTTP ${res.status}`);
    throw new ZwiimClientError("invalid_response", `${op}: HTTP ${res.status}`, res.status);
  }

  openEvents(handlers: EventChannelHandlers): EventChannel {
    let closed = false;
    let stream: ReturnType<Tunnel["openStream"]> | null = null;
    const finish = (code: number, reason: string) => {
      if (closed) return;
      closed = true;
      handlers.onClose({ code, reason });
    };
    this.ensureTunnel().then(
      (tunnel) => {
        if (closed) return;
        stream = tunnel.openStream({
          onOpen: () => {
            if (!closed) handlers.onOpen();
          },
          onMessage: (data) => {
            if (closed) return;
            const frame = decodeServerFrame(data);
            if (frame) handlers.onFrame(frame);
          },
          onClose: ({ code, reason }) => finish(code, reason),
        });
      },
      (e: unknown) => finish(1006, e instanceof Error ? e.message : "connect failed"),
    );
    return {
      send(msg: ClientMessage) {
        if (!closed) stream?.send(JSON.stringify(msg));
      },
      close() {
        const s = stream;
        stream = null;
        finish(1000, "closed by client");
        s?.close();
      },
    };
  }

  /** Close the tunnel. A later request opens a new one. */
  close(): void {
    this.generation++;
    this.connecting = null;
    const tunnel = this.tunnel;
    this.tunnel = null;
    tunnel?.close();
  }

  /** The open tunnel, or a new one. */
  private ensureTunnel(): Promise<Tunnel> {
    if (this.tunnel?.isOpen) return Promise.resolve(this.tunnel);
    if (this.connecting) return this.connecting;
    const generation = this.generation;
    const attempt = (async () => {
      const tunnel = await this.openTunnel();
      if (generation !== this.generation) {
        tunnel.close();
        throw new ZwiimClientError("network", "Transport closed");
      }
      this.tunnel = tunnel;
      tunnel.onEnd(() => {
        if (this.tunnel === tunnel) this.tunnel = null;
      });
      return tunnel;
    })();
    this.connecting = attempt;
    const clear = () => {
      if (this.connecting === attempt) this.connecting = null;
    };
    attempt.then(clear, clear);
    return attempt;
  }
}
