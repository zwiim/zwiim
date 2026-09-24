import type {
  EventChannel,
  EventChannelHandlers,
  RequestOptions,
  Transport,
} from "../src/transport/transport";
import type {
  ClientMessage,
  HostFeedEvent,
  HostSnapshot,
  OpName,
  ServerFrame,
} from "../src/types/protocol";

export interface RecordedRequest {
  op: OpName;
  params: unknown;
  options: RequestOptions | undefined;
}

export class FakeChannel implements EventChannel {
  sent: ClientMessage[] = [];
  closed = false;
  constructor(
    private readonly handlers: EventChannelHandlers,
    private readonly onSend: (ch: FakeChannel, msg: ClientMessage) => void = () => {},
  ) {}
  send(msg: ClientMessage): void {
    this.sent.push(msg);
    this.onSend(this, msg);
  }
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.handlers.onClose({ code: 1000, reason: "client" });
  }
  open(): void {
    this.handlers.onOpen();
  }
  frame(frame: ServerFrame): void {
    this.handlers.onFrame(frame);
  }
  drop(): void {
    if (this.closed) return;
    this.closed = true;
    this.handlers.onClose({ code: 1006, reason: "" });
  }
}

type Handler = (params: any, options?: RequestOptions) => unknown | Promise<unknown>;

/** Scriptable transport: per-op handlers, recorded requests and channels. */
export class FakeTransport implements Transport {
  requests: RecordedRequest[] = [];
  channels: FakeChannel[] = [];
  handlers: Partial<Record<OpName, Handler>> = {
    "host.hello": () => ({ host_id: "host-1", protocol: "zwiim/1", version: "0.0.0", owner: null }),
    "agent.list": () => [],
    "workspace.list": () => [],
    "terminal.list": () => [],
    "session.list": () => [],
    "session.history": (p) => ({ session_id: p.session_id, turns: [] }),
    "session.options": (p) => ({ session_id: p.session_id, options: { model: null, effort: null, mode: null } }),
  };

  async request(op: OpName, params: unknown, options?: RequestOptions): Promise<unknown> {
    this.requests.push({ op, params, options });
    const h = this.handlers[op];
    if (!h) throw new Error(`no handler for ${op}`);
    return await h(params, options);
  }

  /** Host feed: `host.subscribe` is answered with a snapshot built from the list handlers. */
  feedEpoch = "e1";
  feedSeq = 0;
  /** Set false to leave `host.subscribe` unanswered. */
  answerFeed = true;

  snapshot(): HostSnapshot {
    const call = (op: OpName, params: unknown) => this.handlers[op]!(params) as any;
    const workspaces = call("workspace.list", {});
    return {
      epoch: this.feedEpoch,
      seq: this.feedSeq,
      workspaces,
      sessions: workspaces.flatMap((w: { workspace_id: string }) =>
        call("session.list", { workspace_id: w.workspace_id }),
      ),
      terminals: call("terminal.list", {}),
    };
  }

  /** Publish the next feed event on the current channel. */
  feedEvent<T extends HostFeedEvent["type"]>(
    type: T,
    payload: Extract<HostFeedEvent, { type: T }>["payload"],
  ): void {
    const event = { seq: ++this.feedSeq, type, payload } as HostFeedEvent;
    this.channel.frame({ type: "host.event", epoch: this.feedEpoch, event });
  }

  openEvents(handlers: EventChannelHandlers): EventChannel {
    const ch = new FakeChannel(handlers, (c, msg) => {
      if (msg.op === "host.subscribe" && this.answerFeed) {
        c.frame({ type: "host.snapshot", ...this.snapshot() });
      }
    });
    this.channels.push(ch);
    return ch;
  }

  get channel(): FakeChannel {
    const ch = this.channels[this.channels.length - 1];
    if (!ch) throw new Error("no channel opened");
    return ch;
  }

  count(op: OpName): number {
    return this.requests.filter((r) => r.op === op).length;
  }
}

/** Let pending promise continuations run. */
export async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}
