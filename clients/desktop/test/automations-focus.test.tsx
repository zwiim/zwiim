import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HostClient, HostController, ZwiimClientError, type PendingApproval, type RunItemView } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { FocusPanel } from "@/panels/automations/FocusPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

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

  it("a working item says it is working", async () => {
    const c = await mount(transport([]), item({ status: "working" }));
    expect(screen.getByTestId("focus-panel").textContent).toContain("Working on it. The page is live.");
    c.dispose();
  });
});
