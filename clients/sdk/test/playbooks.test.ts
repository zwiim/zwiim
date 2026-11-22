import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
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

    t.transport.handlers["playbook.files"] = () => ({ files: [] });
    expect(await t.controller.listPlaybookFiles("pb1")).toEqual({ files: [] });
    expect(sent()).toMatchObject({ op: "playbook.files", params: { playbook_id: "pb1" } });

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
