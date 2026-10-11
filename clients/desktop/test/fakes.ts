import { act } from "@testing-library/react";
import { CLIENT_CAPABILITIES, CLIENT_REVISION } from "@zwiim/sdk";
import type {
  ClientMessage,
  CloneJob,
  EventChannel,
  EventChannelHandlers,
  HostFeedEvent,
  HostHello,
  OpName,
  ServerFrame,
  Transport,
} from "@zwiim/sdk";

/** A current Host's `host.hello`: same protocol revision, every capability. */
export function hostHello(overrides: Partial<HostHello> = {}): HostHello {
  return {
    host_id: "h1",
    protocol: "zwiim/1",
    version: "0",
    owner: null,
    revision: CLIENT_REVISION,
    min_client_revision: CLIENT_REVISION,
    capabilities: [...CLIENT_CAPABILITIES],
    ...overrides,
  };
}

export const tick = () =>
  act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });

export class FakeTransport implements Transport {
  calls: { op: OpName; params: any }[] = [];
  handlers: Partial<Record<OpName, (p: any) => unknown>> = {
    "host.hello": () => hostHello(),
    "agent.list": () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported", options: null },
      { agent: "codex", name: "Codex", available: false, reason: "not_installed", support: "supported", options: null },
    ],
    "session.options": (p) => ({ session_id: p.session_id, options: null }),
    "workspace.list": () => [],
    "terminal.list": () => [],
    "session.list": () => [],
    "session.history": (p) => ({ session_id: p.session_id, turns: [] }),
  };
  handlersForEvents: EventChannelHandlers | null = null;
  sent: ClientMessage[] = [];
  feedSeq = 0;
  clones: CloneJob[] = [];
  async request(op: OpName, params: unknown) {
    this.calls.push({ op, params });
    const h = this.handlers[op];
    if (!h) throw new Error(`unexpected ${op}`);
    return h(params);
  }
  openEvents(handlers: EventChannelHandlers): EventChannel {
    this.handlersForEvents = handlers;
    queueMicrotask(() => handlers.onOpen());
    return {
      send: (m) => {
        this.sent.push(m);
        // The host answers a feed subscription with what its list operations return.
        if (m.op === "host.subscribe") this.frame({ type: "host.snapshot", ...this.snapshot() });
      },
      close: () => {},
    };
  }
  frame(f: ServerFrame) {
    this.handlersForEvents!.onFrame(f);
  }
  snapshot() {
    const h = this.handlers as Record<string, (p: unknown) => any>;
    const workspaces = h["workspace.list"]!({});
    return {
      epoch: "e1",
      seq: this.feedSeq,
      workspaces,
      sessions: workspaces.flatMap((w: { workspace_id: string }) =>
        h["session.list"]!({ workspace_id: w.workspace_id }).map((s: object) => ({
          ...s,
          workspace_id: w.workspace_id,
        })),
      ),
      terminals: h["terminal.list"]!({}),
      clones: this.clones,
    };
  }
  /** Publish the next host feed event, as another client's change would. */
  feedEvent<T extends HostFeedEvent["type"]>(type: T, payload: Extract<HostFeedEvent, { type: T }>["payload"]) {
    const event = { seq: ++this.feedSeq, type, payload } as HostFeedEvent;
    return act(async () => this.frame({ type: "host.event", epoch: "e1", event }));
  }
}
