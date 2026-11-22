import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type BrowserInput, type BrowserStreamHandlers, type RunItemView } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { ItemTile } from "@/panels/automations/ItemTile";
import { FakeTransport, tick } from "./fakes";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

class StreamingTransport extends FakeTransport {
  streams: { id: string; handlers: BrowserStreamHandlers; sent: BrowserInput[]; closed: boolean }[] = [];
  openBrowserStream(id: string, handlers: BrowserStreamHandlers) {
    const s = { id, handlers, sent: [] as BrowserInput[], closed: false };
    this.streams.push(s);
    return { send: (i: BrowserInput) => void s.sent.push(i), close: () => void (s.closed = true) };
  }
}

function item(extra: Partial<RunItemView> = {}): RunItemView {
  return {
    run_id: "r1",
    item_id: "i1",
    title: "Backend Engineer",
    status: "working",
    reason_code: null,
    site: "jobs.example",
    url: "https://jobs.example/jobs/a",
    tab_id: "t1",
    ...extra,
  };
}

async function mount(t: StreamingTransport, it: RunItemView, onOpen?: () => void) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  const view = render(
    <HostProvider value={controller}>
      <ItemTile item={it} onOpen={onOpen} />
    </HostProvider>,
  );
  await tick();
  const rerender = async (next: RunItemView) => {
    view.rerender(
      <HostProvider value={controller}>
        <ItemTile item={next} />
      </HostProvider>,
    );
    await tick();
  };
  return { controller, rerender };
}

function transport() {
  const t = new StreamingTransport();
  t.handlers["browser.view.open"] = () => ({ stream_id: "st1" });
  t.handlers["browser.view.close"] = () => ({});
  return t;
}

describe("item tiles", () => {
  it("a working item streams its tab at tile size", async () => {
    const t = transport();
    const { controller } = await mount(t, item());
    const opens = t.calls.filter((x) => x.op === "browser.view.open");
    expect(opens).toHaveLength(1);
    expect(opens[0]!.params).toMatchObject({ tab_id: "t1", viewport: { width: 156, height: 338 } });
    const scale = opens[0]!.params.viewport.scale;
    expect(scale).toBeGreaterThanOrEqual(1);
    expect(scale).toBeLessThanOrEqual(2);
    expect(t.streams).toHaveLength(1);
    expect(screen.getByLabelText("Page: Backend Engineer").tagName).toBe("CANVAS");
    expect(screen.getByText("jobs.example")).toBeTruthy();
    controller.dispose();
  });

  it("a tab that goes closes its view", async () => {
    const t = transport();
    const { controller, rerender } = await mount(t, item());
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await rerender(item({ tab_id: null, status: "done" }));
    expect(t.calls.filter((x) => x.op === "browser.view.close").map((x) => x.params)).toEqual([{ stream_id: "st1" }]);
    expect(t.streams[0]!.closed).toBe(true);
    expect(screen.queryByLabelText("Page: Backend Engineer")).toBeNull();
    expect(screen.getByTestId("tile-face").textContent).toContain("jobs.example");
    // The closed stream reports its close; with no tab it must not reopen.
    act(() => t.streams[0]!.handlers.onClose?.({ code: 1000, reason: "" } as never));
    await act(async () => vi.advanceTimersByTime(3000));
    await tick();
    expect(t.calls.filter((x) => x.op === "browser.view.open")).toHaveLength(1);
    controller.dispose();
  });

  it("needs-you tiles show the overlay and ring", async () => {
    const t = transport();
    const { controller } = await mount(t, item({ status: "waiting_approval" }));
    expect(screen.getAllByText("Needs your OK").length).toBeGreaterThan(0);
    expect(screen.getByTestId("tile-frame").className).toContain("ring-brand");
    controller.dispose();
  });

  it("clicking a tile opens it", async () => {
    const t = transport();
    const onOpen = vi.fn();
    const { controller } = await mount(t, item(), onOpen);
    fireEvent.click(screen.getByRole("button", { name: /Backend Engineer/ }));
    expect(onOpen).toHaveBeenCalledTimes(1);
    controller.dispose();
  });
});
