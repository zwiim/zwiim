import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HostClient, HostController, ZwiimClientError, type PendingApproval, type RunItemView } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { AutomationsView, resetAutomationsView } from "@/panels/automations/AutomationsView";
import { FocusPanel } from "@/panels/automations/FocusPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(() => {
  cleanup();
  resetAutomationsView();
});

function item(extra: Partial<RunItemView> = {}): RunItemView {
  return {
    run_id: "r1",
    item_id: "i1",
    title: "Backend Engineer",
    status: "waiting_approval",
    reason_code: null,
    site: "jobs.example",
    url: "https://jobs.example/jobs/a",
    tab_id: "t1",
    ...extra,
  };
}

function approval(): PendingApproval {
  return {
    approval_id: "a1",
    run_id: "r1",
    item_id: "i1",
    kind: "submit_application",
    summary: "Submit Ada's application",
    fields: [
      { target: { role: "textbox", name: "Full name" }, control: "text", value: { value_kind: "text", text: "Ada Lovelace" } },
      {
        target: { role: "textbox", name: "Email" },
        control: "text",
        value: { value_kind: "text", text: "ada@example.com" },
        prefilled: true,
      },
      { target: { role: "textbox", name: "Password" }, control: "secret", value: { value_kind: "secret", handle: "{{secret:pw}}" } },
      { target: { role: "button", name: "Resume" }, control: "file", value: { value_kind: "input_file", name: "resume.pdf" } },
    ],
    submit_target: { role: "button", name: "Submit" },
    created_at: "2026-11-21T10:00:00Z",
  };
}

function transport(approvals: PendingApproval[] = [approval()]) {
  const t = new FakeTransport();
  const snapshot = t.snapshot.bind(t);
  t.snapshot = () => ({ ...snapshot(), approvals });
  t.handlers["run.approve"] = (p) => ({ approval_id: p.approval_id, state: "approved" });
  t.handlers["run.reject"] = (p) => ({ approval_id: p.approval_id, state: "rejected" });
  t.handlers["run.answer"] = (p) => ({ item_id: p.item_id, requeued: true });
  t.handlers["run.resolve"] = (p) => ({ item_id: p.item_id, status: p.choice === "sent" ? "done" : "queued" });
  return t;
}

async function mount(t: FakeTransport, it: RunItemView) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <FocusPanel playbookName="Apply to jobs" item={it} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

describe("focus panel", () => {
  it("an approval shows its fields and sends", async () => {
    const t = transport();
    const c = await mount(t, item());
    const panel = screen.getByTestId("focus-panel");
    expect(panel.textContent).toContain("Apply to jobs · jobs.example");
    expect(panel.textContent).toContain("Submit Ada's application");
    expect(panel.textContent).toContain("Ada Lovelace");
    expect(panel.textContent).toContain("Pre-filled");
    expect(panel.textContent).toContain("Saved secret");
    expect(panel.textContent).not.toContain("{{secret:pw}}");
    expect(panel.textContent).toContain("resume.pdf");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "run.approve").map((x) => x.params)).toEqual([{ approval_id: "a1" }]);
    c.dispose();
  });

  it("skip rejects", async () => {
    const t = transport();
    const c = await mount(t, item());
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "run.reject").map((x) => x.params)).toEqual([{ approval_id: "a1" }]);
    c.dispose();
  });

  it("a refused send says why", async () => {
    const t = transport();
    t.handlers["run.approve"] = () => {
      throw new ZwiimClientError("approval_not_pending", "That approval was already answered.", 409);
    };
    const c = await mount(t, item());
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await tick();
    expect(screen.getByRole("alert").textContent).toContain("That approval was already answered.");
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false);
    c.dispose();
  });

  it("an approval cleared elsewhere falls back", async () => {
    const t = transport();
    const c = await mount(t, item());
    await t.feedEvent("run.approval_cleared", { approval_id: "a1", run_id: "r1" });
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    expect(screen.getByTestId("focus-panel").textContent).toContain("Needs your OK");
    c.dispose();
  });

  it("a question takes an answer", async () => {
    const t = transport([]);
    const c = await mount(t, item({ status: "waiting_person", question: "What is your expected salary?" }));
    expect(screen.getByTestId("focus-panel").textContent).toContain("What is your expected salary?");
    const save = screen.getByRole("button", { name: "Save and retry" }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    fireEvent.change(screen.getByRole("textbox", { name: "Your answer" }), { target: { value: "20 LPA" } });
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    await tick();
    expect(t.calls.filter((x) => x.op === "run.answer").map((x) => x.params)).toEqual([
      { item_id: "i1", answer: "20 LPA" },
    ]);
    c.dispose();
  });

  it("a send that may have gone out asks whether it did", async () => {
    const t = transport([]);
    const c = await mount(
      t,
      item({ status: "waiting_person", reason_code: "check_failed", tab_id: null, may_have_sent: true }),
    );
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByTestId("focus-panel").textContent).toContain("It could not confirm the send went through.");
    expect(screen.getByRole("button", { name: "Not sent, redo" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Skip" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "It was sent" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "run.resolve").map((x) => x.params)).toEqual([
      { item_id: "i1", choice: "sent" },
    ]);
    c.dispose();
  });

  it("a page that needed the person tries again or skips", async () => {
    const t = transport([]);
    const c = await mount(t, item({ status: "waiting_person", reason_code: "page_needs_person", may_have_sent: false }));
    expect(screen.queryByRole("button", { name: "It was sent" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "run.resolve").map((x) => x.params)).toEqual([
      { item_id: "i1", choice: "not_sent" },
      { item_id: "i1", choice: "skip" },
    ]);
    c.dispose();
  });

  it("a working item says it is working", async () => {
    const c = await mount(transport([]), item({ status: "working" }));
    expect(screen.getByTestId("focus-panel").textContent).toContain("Working on it. The page is live.");
    c.dispose();
  });
});

describe("focus view", () => {
  function host() {
    const t = new FakeTransport();
    const snapshot = t.snapshot.bind(t);
    t.snapshot = () => ({ ...snapshot(), approvals: [{ ...approval(), item_id: "c" }] });
    t.handlers["playbook.list"] = () => [
      { playbook_id: "pb1", name: "Apply to jobs", version: 1, show_me: true, freeze_learning: false, sites: [] },
    ];
    t.handlers["run.list"] = () => ({
      runs: [
        {
          run_id: "r1",
          playbook_id: "pb1",
          playbook_version: 1,
          trigger: "mac",
          status: "running",
          pause_reason: null,
          outcome: null,
          items_total: 4,
          items_done: 1,
          created_at: "2026-11-21T10:00:00Z",
          ended_at: null,
        },
      ],
      next_cursor: null,
    });
    const all = [
      summary("a", "done"),
      summary("b", "working"),
      summary("c", "waiting_approval"),
      { ...summary("d", "waiting_person"), question: "Expected salary?" },
    ];
    let items = all;
    t.handlers["run.items"] = () => ({ items, next_cursor: null });
    return { t, drop: (id: string) => (items = all.filter((i) => i.item_id !== id)) };
  }

  function summary(id: string, status: RunItemView["status"]) {
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

  async function mountView(t: FakeTransport) {
    const client = new HostClient({ transport: t, random: () => 0.5 });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <AutomationsView />
      </HostProvider>,
    );
    await tick();
    return controller;
  }

  const shown = () => screen.getByTestId("focus-panel").querySelector("h2")!.textContent;
  const key = (k: string, target: Element | Window = window) => fireEvent.keyDown(target, { key: k });

  it("a tile opens focus and Esc closes it", async () => {
    const { t } = host();
    const c = await mountView(t);
    fireEvent.click(screen.getByRole("button", { name: /Job b/ }));
    expect(shown()).toBe("Job b");
    expect(screen.getByTestId("focus-view").textContent).toContain("2 of 4");
    key("Escape");
    expect(screen.queryByTestId("focus-view")).toBeNull();
    expect(screen.getByTestId("playbook-pb1")).toBeTruthy();
    c.dispose();
  });

  it("arrows move through the run's items", async () => {
    const { t } = host();
    const c = await mountView(t);
    fireEvent.click(screen.getByRole("button", { name: /Job b/ }));
    key("ArrowLeft");
    expect(shown()).toBe("Job a");
    key("ArrowLeft");
    expect(shown()).toBe("Job a");
    key("ArrowRight");
    key("ArrowRight");
    key("ArrowRight");
    expect(shown()).toBe("Job d");
    key("ArrowRight");
    expect(shown()).toBe("Job d");
    // Keys typed into the answer stay there.
    key("ArrowLeft", screen.getByRole("textbox", { name: "Your answer" }));
    expect(shown()).toBe("Job d");
    c.dispose();
  });

  it("the need pill opens the waiting item", async () => {
    const { t } = host();
    const c = await mountView(t);
    fireEvent.click(screen.getByRole("button", { name: "2 need you" }));
    expect(shown()).toBe("Job c");
    expect(screen.getByRole("button", { name: "Send" })).toBeTruthy();
    c.dispose();
  });

  it("focus closes when its item is gone", async () => {
    const { t, drop } = host();
    const c = await mountView(t);
    fireEvent.click(screen.getByRole("button", { name: /Job b/ }));
    drop("b");
    await act(async () => t.frame({ type: "host.snapshot", ...t.snapshot() } as never));
    await tick();
    expect(screen.queryByTestId("focus-view")).toBeNull();
    c.dispose();
  });

  it("a new snapshot keeps focus on an item still in the run", async () => {
    const { t } = host();
    const c = await mountView(t);
    fireEvent.click(screen.getByRole("button", { name: /Job b/ }));
    await act(async () => t.frame({ type: "host.snapshot", ...t.snapshot() } as never));
    await tick();
    expect(shown()).toBe("Job b");
    c.dispose();
  });

  it("Esc meant for a dialog over focus leaves focus open", async () => {
    const { t } = host();
    const c = await mountView(t);
    fireEvent.click(screen.getByRole("button", { name: /Job b/ }));
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.appendChild(dialog);
    key("Escape");
    key("ArrowRight");
    expect(shown()).toBe("Job b");
    dialog.remove();
    key("Escape");
    expect(screen.queryByTestId("focus-view")).toBeNull();
    c.dispose();
  });
});
