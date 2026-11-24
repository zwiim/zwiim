import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import type { PlaybookDraft } from "../src/types/protocol";
import { FakeTransport, flush } from "./fakes";

async function live() {
  const transport = new FakeTransport();
  const client = new HostClient({ transport, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await flush();
  transport.channel.open();
  await flush();
  return { transport, controller, store: controller.store };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("playbook and run methods", () => {
  it("loadRunItems fills the store and returns the next cursor", async () => {
    const t = await live();
    t.transport.handlers["run.items"] = () => ({
      items: [
        {
          item_id: "i1",
          site: "jobs.localhost",
          account: "main",
          key: "a",
          title: "Backend Engineer",
          url: "http://jobs.localhost/a",
          status: "working",
          reason_code: null,
          updated_at: "2026-11-21T10:00:00Z",
          tab_id: "tab-1",
          may_have_sent: true,
        },
      ],
      next_cursor: "i1",
    });
    const next = await t.controller.loadRunItems("r1");
    expect(next).toBe("i1");
    expect(t.transport.requests.at(-1)).toMatchObject({ op: "run.items", params: { run_id: "r1" } });
    expect(t.store.getState().runItems.r1?.i1?.tab_id).toBe("tab-1");
    expect(t.store.getState().runItems.r1?.i1?.may_have_sent).toBe(true);

    await t.controller.loadRunItems("r1", "i1");
    expect(t.transport.requests.at(-1)?.params).toEqual({ run_id: "r1", cursor: "i1" });
  });

  it("sends each operation with the Host's parameter names", async () => {
    const t = await live();
    const sent = () => t.transport.requests.at(-1);
    t.transport.handlers["run.approve"] = () => ({ approval_id: "a1", state: "approved" });
    await t.controller.approve("a1");
    expect(sent()).toMatchObject({ op: "run.approve", params: { approval_id: "a1" }, options: { mutating: true } });

    t.transport.handlers["run.reject"] = () => ({ approval_id: "a1", state: "skipped" });
    await t.controller.reject("a1");
    expect(sent()).toMatchObject({ op: "run.reject", params: { approval_id: "a1" } });

    t.transport.handlers["run.answer"] = () => ({ item_id: "i1", requeued: true });
    expect(await t.controller.answer("i1", "20 LPA")).toEqual({ item_id: "i1", requeued: true });
    expect(sent()).toMatchObject({ op: "run.answer", params: { item_id: "i1", answer: "20 LPA" } });

    t.transport.handlers["run.resolve"] = () => ({ item_id: "i1", status: "done" });
    expect(await t.controller.resolve("i1", "sent")).toEqual({ item_id: "i1", status: "done" });
    expect(sent()).toMatchObject({ op: "run.resolve", params: { item_id: "i1", choice: "sent" }, options: { mutating: true } });

    t.transport.handlers["playbook.inputs"] = () => ({ files: [], facts: [] });
    expect(await t.controller.listPlaybookInputs("pb1")).toEqual({ files: [], facts: [] });
    expect(sent()).toMatchObject({ op: "playbook.inputs", params: { playbook_id: "pb1" } });

    t.transport.handlers["playbook.input.set_fact"] = () => ({ playbook: { playbook_id: "pb1" } });
    await t.controller.setPlaybookFact("pb1", "Email", "me@example.com");
    expect(sent()).toMatchObject({
      op: "playbook.input.set_fact",
      params: { playbook_id: "pb1", input_name: "Email", value: "me@example.com" },
      options: { mutating: true },
    });

    t.transport.handlers["playbook.check.set"] = () => ({ playbook: { playbook_id: "pb1" } });
    await t.controller.setPlaybookCheck("pb1", "Order placed");
    expect(sent()).toMatchObject({
      op: "playbook.check.set",
      params: { playbook_id: "pb1", text: "Order placed" },
      options: { mutating: true },
    });
    await t.controller.setPlaybookCheck("pb1", null);
    expect(sent()).toMatchObject({ op: "playbook.check.set", params: { playbook_id: "pb1", text: null } });

    const file = { input_name: "Resume", file: "cv.pdf", size: 2, sha256: "a".repeat(64) };
    t.transport.handlers["playbook.file.set"] = () => ({ playbook: { playbook_id: "pb1" }, file });
    expect((await t.controller.setPlaybookFile("pb1", "Resume", "/Users/me/cv.pdf")).file).toEqual(file);
    expect(sent()).toMatchObject({
      op: "playbook.file.set",
      params: { playbook_id: "pb1", input_name: "Resume", path: "/Users/me/cv.pdf" },
      options: { mutating: true },
    });

    t.transport.handlers["playbook.file.remove"] = () => ({ playbook: { playbook_id: "pb1" } });
    await t.controller.removePlaybookFile("pb1", "Resume");
    expect(sent()).toMatchObject({
      op: "playbook.file.remove",
      params: { playbook_id: "pb1", input_name: "Resume" },
      options: { mutating: true },
    });

    t.transport.handlers["browser.record.start"] = () => ({});
    await t.controller.startRecording("tab1");
    expect(sent()).toMatchObject({ op: "browser.record.start", params: { tab_id: "tab1" }, options: { mutating: true } });

    const recording = { tab_id: "tab1", start_url: "https://jobs.example/", steps: [] };
    t.transport.handlers["browser.record.stop"] = () => ({ recording });
    expect((await t.controller.stopRecording("tab1")).recording).toEqual(recording);
    expect(sent()).toMatchObject({ op: "browser.record.stop", params: { tab_id: "tab1" }, options: { mutating: true } });

    const draft: PlaybookDraft = {
      name: "Apply on jobs.test",
      goal: "Do what I did on jobs.test: Submit.",
      source: { type: "list", urls: ["https://jobs.test/j/1"] },
      sites: ["jobs.test"],
      steps: [{ action: "click", target: "Submit", role: "button", url: "https://jobs.test/j/1", value: { kind: "none" } }],
      send_step: 0,
      send_kind: "submit_form",
      check_text: null,
    };
    t.transport.handlers["playbook.create_from_draft"] = () => ({ playbook: { playbook_id: "pb2" } });
    await t.controller.createPlaybookFromDraft(draft);
    expect(sent()).toMatchObject({ op: "playbook.create_from_draft", params: { draft }, options: { mutating: true } });

    t.transport.handlers["run.list"] = () => ({ runs: [], next_cursor: null });
    await t.controller.listRuns({ playbookId: "pb1", limit: 5 });
    expect(sent()).toMatchObject({ op: "run.list", params: { playbook_id: "pb1", limit: 5 } });

    t.transport.handlers["run.start"] = () => ({ run_id: "r2" });
    expect(await t.controller.startRun("pb1")).toBe("r2");
    expect(sent()).toMatchObject({ op: "run.start", params: { playbook_id: "pb1" } });

    t.transport.handlers["site.risk_accept"] = () => ({ accepted: true });
    await t.controller.acceptSiteRisk("jobs.localhost");
    expect(sent()).toMatchObject({ op: "site.risk_accept", params: { site: "jobs.localhost" } });
  });
});
