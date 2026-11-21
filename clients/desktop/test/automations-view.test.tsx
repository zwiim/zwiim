import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  HostClient,
  HostController,
  ZwiimClientError,
  type ItemSummary,
  type PlaybookSummary,
  type RunSummary,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { AutomationsView, resetAutomationsView } from "@/panels/automations/AutomationsView";
import { FakeTransport, tick } from "./fakes";

afterEach(() => {
  cleanup();
  resetAutomationsView();
});

async function mount(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  const view = render(
    <HostProvider value={controller}>
      <AutomationsView />
    </HostProvider>,
  );
  await tick();
  return { controller, view };
}

function playbook(id: string, sites: PlaybookSummary["sites"] = []): PlaybookSummary {
  return { playbook_id: id, name: `Playbook ${id}`, version: 1, show_me: true, freeze_learning: false, sites };
}

function run(id: string, playbook_id: string, status: RunSummary["status"]): RunSummary {
  return {
    run_id: id,
    playbook_id,
    playbook_version: 1,
    trigger: "mac",
    status,
    pause_reason: null,
    outcome: null,
    items_total: 3,
    items_done: 1,
    created_at: "2026-11-21T10:00:00Z",
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

function transport(r1: RunSummary["status"] = "running") {
  const t = new FakeTransport();
  t.handlers["playbook.list"] = () => [
    playbook("pb1", [{ site: "jobs.example", risk_accepted: true }]),
    playbook("pb2", [{ site: "forum.example", risk_accepted: false }]),
  ];
  t.handlers["run.list"] = () => ({ runs: [run("r1", "pb1", r1)], next_cursor: null });
  t.handlers["run.items"] = () => ({
    items: [item("a", "done"), item("b", "working"), item("c", "queued")],
    next_cursor: null,
  });
  return t;
}

const section = (id: string) => screen.getByTestId(`playbook-${id}`);

describe("Automations canvas", () => {
  it("a running playbook opens, an idle one is one quiet line", async () => {
    const { controller } = await mount(transport());
    expect(screen.getByTestId("automations-header").textContent).toContain("1 running · 2 playbooks");
    expect(section("pb1").textContent).toContain("1/3 sent");
    expect(within(section("pb1")).getByText("Job b")).toBeTruthy();
    expect(within(section("pb1")).getByRole("button", { name: "Pause" })).toBeTruthy();
    expect(section("pb2").textContent).toContain("Not run yet");
    expect(within(section("pb2")).queryByTestId("tile-strip")).toBeNull();
    controller.dispose();
  });

  it("the person's toggle wins", async () => {
    const first = await mount(transport());
    fireEvent.click(within(section("pb1")).getByRole("button", { name: /Playbook pb1/ }));
    expect(within(section("pb1")).queryByText("Job b")).toBeNull();
    first.view.unmount();
    first.controller.dispose();

    const again = await mount(transport());
    expect(within(section("pb1")).queryByText("Job b")).toBeNull();
    expect(section("pb1").textContent).toContain("Running · 1 of 3 sent");
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(within(section("pb1")).getByText("Job b")).toBeTruthy();
    expect(section("pb2").textContent).toContain("Start it to watch each item here as it runs.");
    expect(screen.getByRole("button", { name: "Collapse all" })).toBeTruthy();
    again.controller.dispose();
  });

  it("start asks for the risk first", async () => {
    const t = transport();
    t.handlers["site.risk_accept"] = () => ({ accepted: true });
    t.handlers["run.start"] = () => ({ run_id: "r9" });
    const { controller } = await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    fireEvent.click(within(section("pb2")).getByRole("button", { name: "Start" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toContain("Start Playbook pb2?");
    expect(dialog.textContent).toContain("forum.example");
    expect(t.calls.some((x) => x.op === "run.start")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "I accept the risk, start" }));
    await tick();
    const ops = t.calls.filter((x) => x.op === "site.risk_accept" || x.op === "run.start");
    expect(ops).toEqual([
      { op: "site.risk_accept", params: { site: "forum.example" } },
      { op: "run.start", params: { playbook_id: "pb2" } },
    ]);
    controller.dispose();
  });

  it("a refused start says why", async () => {
    const t = transport("done");
    t.handlers["run.start"] = () => {
      throw new ZwiimClientError("run_active", "A run of this playbook is already going.", 409);
    };
    const { controller } = await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    fireEvent.click(within(section("pb1")).getByRole("button", { name: "Run again" }));
    await tick();
    expect(within(section("pb1")).getByRole("alert").textContent).toContain("A run of this playbook is already going.");
    controller.dispose();
  });

  it("Labs off offers to turn it on", async () => {
    const t = transport();
    t.handlers["playbook.list"] = () => {
      throw new ZwiimClientError("playbooks_disabled", "off", 409);
    };
    t.handlers["settings.update"] = (p) => p;
    const { controller } = await mount(t);
    expect(screen.getByText("Automations are off")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Turn them on" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "settings.update").map((x) => x.params)).toEqual([
      { values: { "playbooks.enabled": true } },
    ]);
    controller.dispose();
  });
});
