import { describe, expect, it, vi } from "vitest";
import { base64Url, decodeResult, decodeServerFrame, normalizeToolStatus } from "../src/transport/codec";
import { LocalTransport } from "../src/transport/local";
import { ZwiimClientError } from "../src/client/errors";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function rejection(p: Promise<unknown>): Promise<ZwiimClientError> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(ZwiimClientError);
    return e as ZwiimClientError;
  }
  throw new Error("expected rejection");
}

describe("codec", () => {
  it("decodes V1 frames and rejects everything else", () => {
    expect(decodeServerFrame('{"type":"pong"}')).toEqual({ type: "pong" });
    const ev = decodeServerFrame(
      JSON.stringify({
        type: "event",
        subscription_id: "s1",
        event: { session_id: "a", seq: 3, type: "agent.message_delta", payload: { text: "hi" } },
      }),
    );
    expect(ev?.type).toBe("event");
    expect(decodeServerFrame("not json")).toBeNull();
    expect(decodeServerFrame("[]")).toBeNull();
    expect(decodeServerFrame('{"type":"conversation://changed"}')).toBeNull();
    expect(decodeServerFrame("null")).toBeNull();
  });

  describe("tool status", () => {
    // The host spells ACP's in_progress as "inprogress"; agents' own
    // transcripts may already say "in_progress". Both decode the same.
    const WIRE = ["inprogress", "in_progress"] as const;

    it("has one canonical spelling", () => {
      for (const s of WIRE) expect(normalizeToolStatus(s)).toBe("in_progress");
      for (const s of ["pending", "completed", "failed"]) expect(normalizeToolStatus(s)).toBe(s);
      expect(normalizeToolStatus(null)).toBeNull();
      expect(normalizeToolStatus(undefined)).toBeNull();
    });

    it.each(WIRE)("decodes %s in live events, replays and snapshots", (wire) => {
      const call = { tool_call_id: "t1", title: "Run", kind: "execute", status: wire, content: null };
      const event = (seq: number, type: string, payload: unknown) => ({ session_id: "a", seq, type, payload });

      const live = decodeServerFrame(
        JSON.stringify({ type: "event", subscription_id: "s", event: event(1, "agent.tool_call", call) }),
      );
      expect(live?.type === "event" && (live.event.payload as { status: string }).status).toBe("in_progress");

      const replay = decodeServerFrame(
        JSON.stringify({
          type: "replay",
          subscription_id: "s",
          session_id: "a",
          epoch: "e",
          high_water_seq: 2,
          events: [event(1, "agent.tool_call", call), event(2, "agent.tool_call_update", { ...call, kind: undefined })],
        }),
      );
      expect(replay?.type === "replay" && replay.events.map((e) => (e.payload as { status: string }).status)).toEqual([
        "in_progress",
        "in_progress",
      ]);

      const snap = decodeServerFrame(
        JSON.stringify({
          type: "snapshot",
          subscription_id: "s",
          session_id: "a",
          epoch: "e",
          event_seq: 1,
          snapshot: { status: "prompting", tool_calls: [call, { ...call, tool_call_id: "t2", status: "completed" }] },
        }),
      );
      expect(snap?.type === "snapshot" && snap.snapshot.tool_calls.map((t) => t.status)).toEqual([
        "in_progress",
        "completed",
      ]);
    });

    it.each(WIRE)("decodes %s in session history", (wire) => {
      const history = decodeResult("session.history", {
        session_id: "a",
        turns: [
          {
            turn_id: "t",
            role: "assistant",
            timestamp: "2026-09-26T00:00:00Z",
            completed_at: null,
            duration_ms: null,
            model: null,
            usage: null,
            blocks: [
              { type: "text", text: "hi" },
              { type: "tool_call", tool_call_id: "c", title: "Bash", input_preview: null, status: wire },
            ],
          },
        ],
      });
      const block = history.turns[0]!.blocks[1]!;
      expect(block.type === "tool_call" && block.status).toBe("in_progress");
    });
  });

  it("base64url-encodes without padding", () => {
    expect(base64Url("smoke-token-123")).toBe("c21va2UtdG9rZW4tMTIz");
    expect(base64Url("??>")).toBe("Pz8-");
  });
});

describe("LocalTransport.request", () => {
  it("POSTs JSON to /zwiim/v1/<op> with the bearer token", async () => {
    const fetch = vi.fn(async () => jsonResponse(200, [{ workspace_id: "w1" }]));
    const t = new LocalTransport({ url: "http://127.0.0.1:3080/", token: "tok", fetch });
    await expect(t.request("workspace.list", {})).resolves.toEqual([{ workspace_id: "w1" }]);
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:3080/zwiim/v1/workspace.list");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer tok");
    expect(init.body).toBe("{}");
  });

  it("maps host errors to their code", async () => {
    const fetch = vi.fn(async () =>
      jsonResponse(401, { error: { code: "unauthenticated", message: "Missing or invalid token" } }),
    );
    const t = new LocalTransport({ url: "http://h", token: "bad", fetch });
    const e = await rejection(t.request("host.hello", {}));
    expect(e.code).toBe("unauthenticated");
    expect(e.status).toBe(401);
  });

  it("reports an unreachable host as network for reads", async () => {
    const fetch = vi.fn(async () => {
      throw new TypeError("fetch failed");
    });
    const t = new LocalTransport({ url: "http://h", token: "t", fetch });
    expect((await rejection(t.request("workspace.list", {}))).code).toBe("network");
  });

  it("reports a lost mutating request as unknown_outcome", async () => {
    const fetch = vi.fn(async () => {
      throw new TypeError("socket hang up");
    });
    const t = new LocalTransport({ url: "http://h", token: "t", fetch });
    const e = await rejection(
      t.request("session.send", { session_id: "s", blocks: [] }, { mutating: true }),
    );
    expect(e.code).toBe("unknown_outcome");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("treats a non-V1 error body on a mutation as unknown_outcome", async () => {
    const fetch = vi.fn(async () => new Response("Bad Gateway", { status: 502 }));
    const t = new LocalTransport({ url: "http://h", token: "t", fetch });
    const e = await rejection(t.request("terminal.write", {}, { mutating: true }));
    expect(e.code).toBe("unknown_outcome");
  });

  it("times out reads", async () => {
    const fetch = vi.fn(
      (_url: string, init: RequestInit) =>
        new Promise<Response>((_, reject) =>
          init.signal!.addEventListener("abort", () => reject(new Error("aborted"))),
        ),
    );
    const t = new LocalTransport({ url: "http://h", token: "t", fetch: fetch as any });
    const e = await rejection(t.request("workspace.list", {}, { timeoutMs: 5 }));
    expect(e.code).toBe("timeout");
  });
});

class FakeWebSocket {
  static last: FakeWebSocket;
  readyState = 0;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: ((ev: { code: number; reason: string }) => void) | null = null;
  constructor(
    readonly url: string,
    readonly protocols: string[],
  ) {
    FakeWebSocket.last = this;
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({ code: 1000, reason: "" });
  }
}

describe("LocalTransport.openEvents", () => {
  it("authenticates with subprotocols and decodes frames", () => {
    const t = new LocalTransport({
      url: "http://127.0.0.1:3080",
      token: "smoke-token-123",
      WebSocket: FakeWebSocket as any,
    });
    const frames: unknown[] = [];
    const onOpen = vi.fn();
    const onClose = vi.fn();
    const ch = t.openEvents({ onOpen, onFrame: (f) => frames.push(f), onClose });
    const ws = FakeWebSocket.last;
    expect(ws.url).toBe("ws://127.0.0.1:3080/zwiim/v1/ws");
    expect(ws.protocols).toEqual(["zwiim-events", "zwiim-token.c21va2UtdG9rZW4tMTIz"]);

    ch.send({ op: "ping" }); // not open yet: dropped
    ws.readyState = 1;
    ws.onopen?.();
    expect(onOpen).toHaveBeenCalled();
    ch.send({ op: "ping" });
    expect(ws.sent).toEqual(['{"op":"ping"}']);

    ws.onmessage?.({ data: '{"type":"pong"}' });
    ws.onmessage?.({ data: "garbage" });
    expect(frames).toEqual([{ type: "pong" }]);

    ws.onerror?.();
    ws.onclose?.({ code: 1006, reason: "" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
