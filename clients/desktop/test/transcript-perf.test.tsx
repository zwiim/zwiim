// A long transcript stays cheap: only the history rows in and near view are
// mounted, the live turn streaming redraws none of them, and the view follows
// the end (or stays put) as before.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type HistoryTurn, type SessionSnapshot } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, tick } from "./fakes";

// Each history turn drawn: a reply draws its sub-agent cards, a message what it carried.
const drawn = vi.hoisted(() => ({ replies: 0, messages: 0 }));
const fromHistory = (blocks: readonly { type: string; text?: string }[]) =>
  blocks.some((b) => b.type === "text" && b.text?.startsWith("history "));
vi.mock("@zwiim/sdk", async (load) => {
  const sdk = await load<typeof import("@zwiim/sdk")>();
  return {
    ...sdk,
    historySubagentCards: (...args: Parameters<typeof sdk.historySubagentCards>) => {
      if (fromHistory(args[0])) drawn.replies++;
      return sdk.historySubagentCards(...args);
    },
  };
});
vi.mock("@/attachments/AttachmentChip", async (load) => {
  const chips = await load<typeof import("@/attachments/AttachmentChip")>();
  return {
    ...chips,
    SentAttachments: (props: Parameters<typeof chips.SentAttachments>[0]) => {
      if (fromHistory(props.blocks)) drawn.messages++;
      return chips.SentAttachments(props);
    },
  };
});

afterEach(cleanup);

// jsdom lays nothing out. The transcript is VIEW tall, each history row ROW
// tall (and CARD more with a sub-agent card open in it), what follows the
// history `below` tall; its scroll position is kept within what it can
// scroll, and moving it is reported as a scroll, as a browser does.
const VIEW = 600;
const ROW = 100;
const CARD = 900;
const GAP = 20;
let below = 0;
const isTranscript = (el: Element) => (el as HTMLElement).dataset?.testid === "transcript";
const isRow = (el: Element) => el.hasAttribute("data-index");
const rowHeight = (el: Element) =>
  ROW + (el.querySelector('[data-testid="subagent-card"] > button[aria-expanded="true"]') ? CARD : 0);
function contentHeight(el: HTMLElement) {
  const list = el.querySelector<HTMLElement>('[data-testid="history"]');
  let h = below;
  if (list) {
    const rows = [...list.querySelectorAll(":scope > [data-index]")];
    h += (parseFloat(list.style.paddingTop) || 0) + (parseFloat(list.style.paddingBottom) || 0);
    h += rows.reduce((sum, r) => sum + rowHeight(r), 0) + Math.max(0, rows.length - 1) * GAP;
  }
  return Math.max(VIEW, h);
}
// What the resize observers watch, so a test can report a row's new size.
let observed: (() => void)[] = [];
const reportSizes = () => act(async () => observed.forEach((report) => report()));
const offsets = new WeakMap<Element, number>();
const saved: [object, string, PropertyDescriptor | undefined][] = [];
function override(proto: object, name: string, desc: PropertyDescriptor) {
  saved.push([proto, name, Object.getOwnPropertyDescriptor(proto, name)]);
  Object.defineProperty(proto, name, { configurable: true, ...desc });
}
beforeEach(() => {
  below = 0;
  observed = [];
  drawn.replies = 0;
  drawn.messages = 0;
  const height = (el: HTMLElement) => (isTranscript(el) ? VIEW : isRow(el) ? rowHeight(el) : 0);
  const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight")!;
  override(HTMLElement.prototype, "clientHeight", {
    get(this: HTMLElement) {
      return isTranscript(this) ? VIEW : clientHeight.get!.call(this);
    },
  });
  override(HTMLElement.prototype, "offsetHeight", {
    get(this: HTMLElement) {
      return height(this);
    },
  });
  override(HTMLElement.prototype, "scrollHeight", {
    get(this: HTMLElement) {
      return isTranscript(this) ? contentHeight(this) : 0;
    },
  });
  override(HTMLElement.prototype, "scrollTop", {
    get(this: HTMLElement) {
      return offsets.get(this) ?? 0;
    },
    set(this: HTMLElement, v: number) {
      const max = isTranscript(this) ? Math.max(0, contentHeight(this) - VIEW) : 0;
      const next = Math.min(Math.max(0, v), max);
      if (next === (offsets.get(this) ?? 0)) return;
      offsets.set(this, next);
      queueMicrotask(() => this.dispatchEvent(new Event("scroll")));
    },
  });
  override(HTMLElement.prototype, "scrollTo", {
    value(this: HTMLElement, opts: { top?: number }) {
      if (opts.top !== undefined) this.scrollTop = opts.top;
    },
  });
  // Sizes come in after layout, as a browser's resize observer reports them.
  override(globalThis, "ResizeObserver", {
    writable: true,
    value: class {
      constructor(private report: ResizeObserverCallback) {}
      observe(el: HTMLElement) {
        const report = () => {
          if (!el.isConnected) return;
          const size = height(el);
          const entry = { target: el, borderBoxSize: [{ blockSize: size, inlineSize: 0 }], contentRect: { height: size } };
          this.report([entry as unknown as ResizeObserverEntry], this as unknown as ResizeObserver);
        };
        observed.push(report);
        queueMicrotask(report);
      }
      unobserve() {}
      disconnect() {}
    },
  });
});
afterEach(() => {
  for (const [proto, name, desc] of saved.reverse()) {
    if (desc) Object.defineProperty(proto, name, desc);
    else delete (proto as Record<string, unknown>)[name];
  }
  saved.length = 0;
});

const turn = (turn_id: string, role: string, blocks: HistoryTurn["blocks"]): HistoryTurn => ({
  turn_id,
  role,
  blocks,
  timestamp: "",
  completed_at: null,
  duration_ms: null,
  model: null,
  usage: null,
});

/** `count` messages, a message and its reply in turn; the replies `extra` adds to carry more. */
function conversation(count: number, extra: (i: number) => HistoryTurn["blocks"] = () => []): HistoryTurn[] {
  return Array.from({ length: count }, (_, i) =>
    i % 2 === 0
      ? turn(`t${i}`, "user", [{ type: "text", text: `history message ${i}` }])
      : turn(`t${i}`, "assistant", [{ type: "text", text: `history reply ${i}` }, ...extra(i)]),
  );
}

const snapshot = (extra: Partial<SessionSnapshot> = {}): SessionSnapshot => ({
  status: "connected",
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
  ...extra,
});

async function openSession(turns: HistoryTurn[]) {
  const t = new FakeTransport();
  t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns });
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} />
    </HostProvider>,
  );
  await settle();
  const frame = (f: object) => act(async () => t.frame({ subscription_id: "sub-1", session_id: "s1", ...f } as never));
  let seq = 1;
  const event = (type: string, payload: object) =>
    frame({ type: "event", event: { session_id: "s1", seq: ++seq, type, payload } });
  await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot({ status: "prompting" }) });
  await settle();
  return { controller, event, el: screen.getByTestId("transcript") };
}

/** Lets scrolls, measurements and the redraws they cause run out. */
async function settle() {
  for (let i = 0; i < 5; i++) await tick();
}

const mounted = () => screen.getByTestId("history").querySelectorAll(":scope > [data-index]").length;
const end = (el: HTMLElement) => el.scrollHeight - el.clientHeight;
async function scrollTo(el: HTMLElement, top: number) {
  await act(async () => {
    fireEvent.wheel(el);
    el.scrollTop = top;
  });
  await settle();
}

describe("long transcript", () => {
  it("redraws no history turn as the live turn streams", async () => {
    const { controller, event } = await openSession(conversation(30));
    expect(screen.getByText("history reply 29")).toBeTruthy();
    drawn.replies = 0;
    drawn.messages = 0;
    await event("agent.message_delta", { text: "streaming " });
    await event("agent.message_delta", { text: "more " });
    await event("agent.thought_delta", { text: "thinking" });
    await settle();
    expect(screen.getByText(/streaming more/)).toBeTruthy();
    expect(drawn).toEqual({ replies: 0, messages: 0 });
    controller.dispose();
  });

  it("mounts a few rows of a 5,000-message history, and opens at its end", async () => {
    const { controller, el } = await openSession(conversation(5000));
    const rows = mounted();
    expect(rows).toBeGreaterThan(0);
    // The view holds 6 rows; a few more either side are kept ready.
    expect(rows).toBeLessThanOrEqual(20);
    expect(screen.getByText("history reply 4999")).toBeTruthy();
    expect(screen.queryByText("history message 0")).toBeNull();
    expect(el.scrollTop).toBe(end(el));
    // Scrolled to the top, the first rows replace the last.
    await scrollTo(el, 0);
    expect(screen.getByText("history message 0")).toBeTruthy();
    expect(screen.queryByText("history reply 4999")).toBeNull();
    expect(mounted()).toBeLessThanOrEqual(20);
    controller.dispose();
  });

  it("follows new live content at the end, and leaves a reader who scrolled up where they are", async () => {
    const { controller, event, el } = await openSession(conversation(200));
    expect(el.scrollTop).toBe(end(el));
    below = 150;
    await event("agent.message_delta", { text: "new words" });
    await vi.waitFor(() => expect(el.scrollTop).toBe(end(el)), { timeout: 3000 });

    // Rows measured on the way keep what is in view where it is.
    await scrollTo(el, 4000);
    const at = el.scrollTop;
    expect(at).toBeLessThan(end(el) - VIEW);
    const first = document.querySelector("[data-index]")!.getAttribute("data-index");
    below = 400;
    await event("agent.message_delta", { text: " and more" });
    await event("agent.tool_call", { tool_call_id: "k1", title: "Read a.txt", kind: "read", status: "completed", content: null });
    await settle();
    expect(el.scrollTop).toBe(at);
    expect(document.querySelector("[data-index]")!.getAttribute("data-index")).toBe(first);
    controller.dispose();
  });

  it("jumps to the true end, the live turn included", async () => {
    const { controller, event, el } = await openSession(conversation(200));
    await scrollTo(el, 1000);
    const at = el.scrollTop;
    below = 300;
    await event("agent.message_delta", { text: "live reply" });
    await settle();
    expect(el.scrollTop).toBe(at);
    fireEvent.click(screen.getByText("Jump to latest"));
    await vi.waitFor(() => expect(el.scrollTop).toBe(end(el)), { timeout: 3000 });
    await settle();
    expect(el.scrollTop).toBe(end(el));
    expect(screen.getByText("history reply 199")).toBeTruthy();
    expect(screen.getByText("live reply")).toBeTruthy();
    expect(screen.queryByText("Jump to latest")).toBeNull();
    controller.dispose();
  });

  it("leaves the view where it is when the reader opens a card at the end, and follows the live turn after", async () => {
    const card = (i: number): HistoryTurn["blocks"] =>
      i === 199
        ? [
            {
              type: "tool_call",
              tool_call_id: "a1",
              title: "Task",
              input_preview: null,
              status: "completed",
              subagent: { description: "Survey the repo", blocks: [{ type: "text", text: "surveying" }] },
            },
          ]
        : [];
    const { controller, event, el } = await openSession(conversation(200, card));
    // Opened at the end, with the rows measured on the way.
    expect(el.scrollTop).toBe(end(el));
    const at = el.scrollTop;
    const header = screen.getByTestId("subagent-card").querySelector<HTMLButtonElement>(":scope > button")!;
    fireEvent.pointerDown(header);
    fireEvent.click(header);
    await reportSizes();
    await settle();
    expect(screen.getByText("surveying")).toBeTruthy();
    // The row grew by more than the view; the card's header stays where it was.
    expect(end(el)).toBeGreaterThan(at + VIEW);
    expect(el.scrollTop).toBe(at);
    // Closing it moves nothing either.
    fireEvent.pointerDown(header);
    fireEvent.click(header);
    await reportSizes();
    await settle();
    expect(el.scrollTop).toBe(at);

    // The live turn growing still keeps a reader at the end there.
    below = 150;
    await event("agent.message_delta", { text: "new words" });
    await vi.waitFor(() => expect(el.scrollTop).toBe(end(el)), { timeout: 3000 });
    controller.dispose();
  });

  it("keeps a card, a tool call and a thought open after they scroll out of view and back", async () => {
    const card = (i: number): HistoryTurn["blocks"] =>
      i === 197
        ? [
            { type: "thought", text: "weighing it" },
            { type: "tool_call", tool_call_id: "c1", title: "Bash", input_preview: '{"command":"ls"}', status: "completed" },
            {
              type: "tool_call",
              tool_call_id: "a1",
              title: "Task",
              input_preview: null,
              status: "completed",
              subagent: { description: "Survey the repo", blocks: [{ type: "text", text: "surveying" }] },
            },
          ]
        : [];
    const { controller, el } = await openSession(conversation(200, card));
    const toggles = () => {
      const c = screen.getByTestId("subagent-card");
      return {
        card: c.querySelector<HTMLButtonElement>(":scope > button")!,
        tool: within(screen.getByTestId("tool-call")).getAllByRole("button")[0]!,
        thought: within(screen.getByTestId("thought")).getAllByRole("button")[0]!,
      };
    };
    const opened = toggles();
    for (const b of Object.values(opened)) {
      expect(b.getAttribute("aria-expanded")).toBe("false");
      fireEvent.click(b);
    }
    expect(screen.getByText("surveying")).toBeTruthy();

    await scrollTo(el, 0);
    expect(screen.queryByTestId("subagent-card")).toBeNull();
    await scrollTo(el, end(el));
    for (const b of Object.values(toggles())) expect(b.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("surveying")).toBeTruthy();
    controller.dispose();
  });
});
