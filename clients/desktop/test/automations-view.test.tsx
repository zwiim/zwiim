import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
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
import {
  AutomationsView,
  resetAutomationsView,
} from "@/panels/automations/AutomationsView";
import { RecordingProvider } from "@/panels/automations/recording";
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

function playbook(
  id: string,
  sites: PlaybookSummary["sites"] = [],
): PlaybookSummary {
  return {
    playbook_id: id,
    name: `Playbook ${id}`,
    version: 1,
    show_me: true,
    freeze_learning: false,
    sites,
  };
}

function run(
  id: string,
  playbook_id: string,
  status: RunSummary["status"],
): RunSummary {
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
  t.handlers["run.list"] = () => ({
    runs: [run("r1", "pb1", r1)],
    next_cursor: null,
  });
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
    expect(screen.getByTestId("automations-header").textContent).toContain(
      "1 running · 2 playbooks",
    );
    expect(section("pb1").textContent).toContain("1/3 sent");
    expect(within(section("pb1")).getByText("Job b")).toBeTruthy();
    expect(
      within(section("pb1")).getByRole("button", { name: "Pause" }),
    ).toBeTruthy();
    expect(section("pb2").textContent).toContain("Not run yet");
    expect(within(section("pb2")).queryByTestId("tile-strip")).toBeNull();
    controller.dispose();
  });

  it("the person's toggle wins", async () => {
    const first = await mount(transport());
    fireEvent.click(
      within(section("pb1")).getByRole("button", { name: /Playbook pb1/ }),
    );
    expect(within(section("pb1")).queryByText("Job b")).toBeNull();
    first.view.unmount();
    first.controller.dispose();

    const again = await mount(transport());
    expect(within(section("pb1")).queryByText("Job b")).toBeNull();
    expect(section("pb1").textContent).toContain("Running · 1 of 3 sent");
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(within(section("pb1")).getByText("Job b")).toBeTruthy();
    expect(section("pb2").textContent).toContain(
      "Start it to watch each item here as it runs.",
    );
    expect(screen.getByRole("button", { name: "Collapse all" })).toBeTruthy();
    again.controller.dispose();
  });

  it("start asks for the risk first", async () => {
    const t = transport();
    t.handlers["site.risk_accept"] = () => ({ accepted: true });
    t.handlers["run.start"] = () => ({ run_id: "r9" });
    const { controller } = await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    fireEvent.click(
      within(section("pb2")).getByRole("button", { name: "Start" }),
    );
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toContain("Start Playbook pb2?");
    expect(dialog.textContent).toContain("forum.example");
    expect(t.calls.some((x) => x.op === "run.start")).toBe(false);
    fireEvent.click(
      within(dialog).getByRole("button", { name: "I accept the risk, start" }),
    );
    await tick();
    const ops = t.calls.filter(
      (x) => x.op === "site.risk_accept" || x.op === "run.start",
    );
    expect(ops).toEqual([
      { op: "site.risk_accept", params: { site: "forum.example" } },
      { op: "run.start", params: { playbook_id: "pb2" } },
    ]);
    controller.dispose();
  });

  it("a refused start says why", async () => {
    const t = transport("done");
    t.handlers["run.start"] = () => {
      throw new ZwiimClientError(
        "run_active",
        "A run of this playbook is already going.",
        409,
      );
    };
    const { controller } = await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    fireEvent.click(
      within(section("pb1")).getByRole("button", { name: "Run again" }),
    );
    await tick();
    expect(within(section("pb1")).getByRole("alert").textContent).toContain(
      "A run of this playbook is already going.",
    );
    controller.dispose();
  });

  it("an open failed run says why", async () => {
    const t = transport("failed");
    t.handlers["run.list"] = () => ({
      runs: [{ ...run("r1", "pb1", "failed"), error: "agent_unavailable" }],
      next_cursor: null,
    });
    t.handlers["run.items"] = () => ({ items: [], next_cursor: null });
    const { controller } = await mount(t);
    expect(section("pb1").textContent).toContain("Failed");
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(section("pb1").textContent).toContain(
      "Failed · The agent could not start",
    );
    controller.dispose();
  });

  it("a run that found only items done before says nothing was new", async () => {
    const t = transport("done");
    t.handlers["run.list"] = () => ({
      runs: [{ ...run("r1", "pb1", "done"), outcome: "nothing_new" }],
      next_cursor: null,
    });
    t.handlers["run.items"] = () => ({ items: [], next_cursor: null });
    const { controller } = await mount(t);
    expect(section("pb1").textContent).toContain("Nothing new");
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(section("pb1").textContent).toContain(
      "Nothing new: every item was done before",
    );
    controller.dispose();
  });

  it("a site paused for sign-in says so and resumes", async () => {
    const t = transport();
    let paused = true;
    t.handlers["site.list"] = () => [
      {
        site: "jobs.example",
        account: "main",
        paused_cause: paused ? "login" : null,
        risk_accepted: true,
      },
      {
        site: "elsewhere.example",
        account: "main",
        paused_cause: "captcha",
        risk_accepted: true,
      },
    ];
    t.handlers["site.resume"] = () => {
      paused = false;
      return { resumed: true };
    };
    const { controller } = await mount(t);
    const banner = screen.getByTestId("paused-sites");
    expect(banner.textContent).toContain("jobs.example is paused");
    expect(banner.textContent).toContain("Sign in to it in the Browser panel");
    expect(banner.textContent).not.toContain("elsewhere.example");
    fireEvent.click(within(banner).getByRole("button", { name: "Resume" }));
    await tick();
    expect(
      t.calls.filter((x) => x.op === "site.resume").map((x) => x.params),
    ).toEqual([{ site: "jobs.example" }]);
    expect(screen.queryByTestId("paused-sites")).toBeNull();
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
    expect(
      t.calls.filter((x) => x.op === "settings.update").map((x) => x.params),
    ).toEqual([{ values: { "playbooks.enabled": true } }]);
    controller.dispose();
  });

  it("with no playbooks, offers to record one or write one in chat", async () => {
    const t = new FakeTransport();
    t.handlers["playbook.list"] = () => [];
    t.handlers["run.list"] = () => ({ runs: [], next_cursor: null });
    t.handlers["browser.tab.open"] = () => ({
      tab_id: "mine1",
      owner: { kind: "personal" },
      url: "about:blank",
      title: "",
      position: 0,
      loading: false,
      discarded: false,
      crashed: false,
    });
    t.handlers["browser.record.start"] = () => ({});
    const client = new HostClient({ transport: t, random: () => 0.5 });
    const controller = new HostController(client);
    client.start();
    await tick();
    let chat = 0;
    let shown = 0;
    render(
      <HostProvider value={controller}>
        <RecordingProvider
          canShowBrowser
          onShowBrowser={() => (shown += 1)}
          onReview={() => {}}
        >
          <AutomationsView onChat={() => (chat += 1)} />
        </RecordingProvider>
      </HostProvider>,
    );
    await tick();
    expect(
      screen.getByText("No playbooks yet. Do a task once and Zwiim saves it."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Write one in chat" }));
    expect(chat).toBe(1);
    fireEvent.click(screen.getByRole("button", { name: "Record a playbook" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "browser.record.start")).toHaveLength(
      1,
    );
    expect(shown).toBe(1);
    controller.dispose();
  });

  it("records another playbook from the header", async () => {
    const t = transport();
    const client = new HostClient({ transport: t, random: () => 0.5 });
    const controller = new HostController(client);
    client.start();
    await tick();
    let asked = 0;
    render(
      <HostProvider value={controller}>
        <RecordingProvider
          canShowBrowser={false}
          onShowBrowser={() => {}}
          onReview={() => {}}
        >
          <AutomationsView />
        </RecordingProvider>
      </HostProvider>,
    );
    await tick();
    fireEvent.click(
      within(screen.getByTestId("automations-header")).getByRole("button", {
        name: "Record",
      }),
    );
    await tick();
    asked += 1;
    expect(screen.getByRole("alert").textContent).toContain(
      "Add a project folder first",
    );
    expect(asked).toBe(1);
    controller.dispose();
  });
});
