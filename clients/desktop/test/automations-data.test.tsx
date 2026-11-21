import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HostClient, HostController, ZwiimClientError, type ItemSummary, type PlaybookSummary, type RunSummary } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { useAutomations, type Automations } from "@/panels/automations/data";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

let latest: Automations | null = null;

function Probe() {
  const a = useAutomations();
  latest = a;
  const rows = a.rows.map((r) => ({
    pb: r.playbook.playbook_id,
    run: r.run?.run_id ?? null,
    items: r.items.map((i) => `${i.item_id}:${i.status}`),
  }));
  return <pre data-testid="probe">{JSON.stringify({ state: a.state, rows })}</pre>;
}

function read(): { state: string; rows: { pb: string; run: string | null; items: string[] }[] } {
  return JSON.parse(screen.getByTestId("probe").textContent!);
}

async function mount(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Probe />
    </HostProvider>,
  );
  await tick();
  return controller;
}

function playbook(id: string): PlaybookSummary {
  return { playbook_id: id, name: `Playbook ${id}`, version: 1, show_me: true, freeze_learning: false, sites: [] };
}

function run(id: string, playbook_id: string, created_at: string, status: RunSummary["status"] = "done"): RunSummary {
  return {
    run_id: id,
    playbook_id,
    playbook_version: 1,
    trigger: "mac",
    status,
    pause_reason: null,
    outcome: null,
    items_total: 0,
    items_done: 0,
    created_at,
    ended_at: null,
  };
}

function item(id: string, status: ItemSummary["status"]): ItemSummary {
  return {
    item_id: id,
    site: "jobs.example",
    account: "",
    key: `jobs.example/${id}`,
    title: `Job ${id}`,
    url: `https://jobs.example/${id}`,
    status,
    reason_code: null,
    updated_at: "2026-11-21T10:00:00Z",
  };
}

function transport() {
  const t = new FakeTransport();
  t.handlers["playbook.list"] = () => [playbook("pb1"), playbook("pb2")];
  t.handlers["run.list"] = () => ({
    runs: [run("r2", "pb1", "2026-11-21T10:00:00Z", "running"), run("r1", "pb1", "2026-11-20T10:00:00Z")],
    next_cursor: null,
  });
  t.handlers["run.items"] = (p: { run_id: string }) => ({
    items: p.run_id === "r2" ? [item("i1", "working"), item("i2", "queued")] : [],
    next_cursor: null,
  });
  return t;
}

describe("useAutomations", () => {
  it("lists each playbook with its latest run and items", async () => {
    const t = transport();
    const c = await mount(t);
    expect(read()).toEqual({
      state: "ready",
      rows: [
        { pb: "pb1", run: "r2", items: ["i1:working", "i2:queued"] },
        { pb: "pb2", run: null, items: [] },
      ],
    });
    expect(t.calls.filter((x) => x.op === "run.items").map((x) => x.params)).toEqual([{ run_id: "r2" }]);
    c.dispose();
  });

  it("follows run.item_changed live", async () => {
    const t = transport();
    const c = await mount(t);
    await t.feedEvent("run.item_changed", {
      run_id: "r2",
      item_id: "i1",
      title: "Job i1",
      status: "done",
      reason_code: null,
      tab_id: null,
    });
    expect(read().rows[0]!.items).toEqual(["i1:done", "i2:queued"]);
    c.dispose();
  });

  it("reloads items a new snapshot dropped", async () => {
    const t = transport();
    const c = await mount(t);
    await act(async () => t.frame({ type: "host.snapshot", ...t.snapshot() } as never));
    await tick();
    expect(t.calls.filter((x) => x.op === "run.items")).toHaveLength(2);
    expect(read().rows[0]!.items).toEqual(["i1:working", "i2:queued"]);
    c.dispose();
  });

  it("Labs off shows off and turnOn enables", async () => {
    const t = transport();
    let enabled = false;
    const list = t.handlers["playbook.list"]!;
    t.handlers["playbook.list"] = (p) => {
      if (!enabled) throw new ZwiimClientError("playbooks_disabled", "Automations are off", 409);
      return list(p);
    };
    t.handlers["settings.update"] = (p: { values: Record<string, unknown> }) => {
      enabled = p.values["playbooks.enabled"] === true;
      return p;
    };
    const c = await mount(t);
    expect(read().state).toBe("off");
    await act(async () => latest!.turnOn());
    await tick();
    expect(t.calls.filter((x) => x.op === "settings.update").map((x) => x.params)).toEqual([
      { values: { "playbooks.enabled": true } },
    ]);
    expect(t.calls.filter((x) => x.op === "playbook.list")).toHaveLength(2);
    expect(read().state).toBe("ready");
    c.dispose();
  });
});
