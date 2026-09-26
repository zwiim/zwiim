import type { ClientMessage, OpName, ServerFrame } from "../types/protocol";

export interface RequestOptions {
  /** Abort the request after this many milliseconds. */
  timeoutMs?: number;
  /** Operation is not safe to repeat; failures after sending are `unknown_outcome`. */
  mutating?: boolean;
}

export interface EventChannelHandlers {
  onOpen(): void;
  onFrame(frame: ServerFrame): void;
  onClose(info: { code: number; reason: string }): void;
}

/** One multiplexed event socket to a host. */
export interface EventChannel {
  send(msg: ClientMessage): void;
  close(): void;
}

/**
 * How a client reaches one host. `LocalTransport` is localhost HTTP + WS;
 * `RelayTransport` and `LanTransport` are the same contract in the device
 * tunnel, through Zwiim Cloud or directly on the local network, and
 * `ConnectionManager` chooses between those two. Clients never need to know
 * which one they hold.
 */
export interface Transport {
  request(op: OpName, params: unknown, options?: RequestOptions): Promise<unknown>;
  openEvents(handlers: EventChannelHandlers): EventChannel;
  /** Release any connection the transport holds open between calls. */
  close?(): void;
}
