import { describe, expect, it } from "vitest";
import { initialHostState, reduceHost, type HostAction, type HostState } from "../src/state/store";
import type { HostFeedEvent, ItemSummary, PendingApproval, RunItemView, RunView } from "../src/types/protocol";

const run = (id: string, status: RunView["status"] = "running"): RunView => ({
  run_id: id,
  playbook_id: "pb1",
  status,
  pause_reason: null,
  outcome: null,
  error: null,
  totals: { items: 2, done: 0, skipped: 0, failed: 0, deferred: 0, waiting: 0 },
  created_at: "2026-11-21T10:00:00Z",
  ended_at: null,
});

const approval = (id: string): PendingApproval => ({
  approval_id: id,
  run_id: "r1",
  item_id: "i1",
  playbook_name: "Job applications",
  site: "jobs.localhost",
  item_title: "Backend Engineer",
  item_url: null,
  kind: "submit_application",
  summary: "Apply",
  fields: [],
  submit_target: { role: "button", name: "Submit" },
  created_at: "2026-11-21T10:00:00Z",
});

const summary = (id: string, status: ItemSummary["status"]): ItemSummary => ({
  item_id: id,
  site: "jobs.localhost",
  account: "main",
  key: id,
  title: `Job ${id}`,
  url: `http://jobs.localhost/${id}`,
  status,
  reason_code: null,
  updated_at: "2026-11-21T10:00:00Z",
  tab_id: null,
});

const snapshot = (runs: RunView[], approvals: PendingApproval[] = []): HostAction => ({
  type: "host_snapshot",
  snapshot: { epoch: "e", seq: 0, workspaces: [], sessions: [], terminals: [], runs, approvals },
});

let seq = 0;
const event = (s: HostState, e: Omit<HostFeedEvent, "seq">) =>
  reduceHost(s, { type: "host_event", event: { seq: ++seq, ...e } as HostFeedEvent });

describe("runs", () => {
  it("come from the snapshot and follow run.changed", () => {
    let s = reduceHost(initialHostState(), snapshot([run("r1")]));
    expect(s.runs.r1?.status).toBe("running");
    s = event(s, { type: "run.changed", payload: { ...run("r1", "done"), ended_at: "2026-11-21T11:00:00Z" } });
    expect(s.runs.r1?.status).toBe("done");
  });

  it("a new snapshot keeps runs it does not list", () => {
    let s = reduceHost(initialHostState(), snapshot([run("r1")], [approval("a1")]));
    s = event(s, { type: "run.changed", payload: run("r1", "done") });
    s = reduceHost(s, snapshot([run("r2")]));
    expect(Object.keys(s.runs).sort()).toEqual(["r1", "r2"]);
    expect(s.runs.r1?.status).toBe("done");
    // Approvals are what the Host says waits now.
    expect(s.approvals).toEqual({});
  });
});

describe("approvals", () => {
  it("follow requested and cleared", () => {
    let s = reduceHost(initialHostState(), snapshot([], [approval("a1")]));
    expect(s.approvals.a1?.playbook_name).toBe("Job applications");
    s = event(s, { type: "run.approval_requested", payload: approval("a2") });
    expect(Object.keys(s.approvals).sort()).toEqual(["a1", "a2"]);
    s = event(s, { type: "run.approval_cleared", payload: { approval_id: "a1", run_id: "r1" } });
    expect(Object.keys(s.approvals)).toEqual(["a2"]);
  });
});

describe("run items", () => {
  it("merge from pages and item_changed", () => {
    let s = reduceHost(initialHostState(), {
      type: "run_items_loaded",
      run_id: "r1",
      items: [summary("i1", "queued"), { ...summary("i2", "waiting_person"), question: "Salary?" }],
    });
    expect(s.runItems.r1?.i2?.question).toBe("Salary?");
    expect(s.runItems.r1?.i1?.url).toBe("http://jobs.localhost/i1");
    const changed: RunItemView = {
      run_id: "r1",
      item_id: "i1",
      title: "Job i1",
      status: "working",
      reason_code: null,
      site: "jobs.localhost",
      url: "http://jobs.localhost/i1",
      question: null,
      tab_id: "tab-1",
    };
    s = event(s, { type: "run.item_changed", payload: changed });
    expect(s.runItems.r1?.i1).toEqual(changed);
    expect(s.runItems.r1?.i2?.status).toBe("waiting_person");
  });

  it("an item_changed for a run not loaded yet starts its list", () => {
    const s = event(initialHostState(), {
      type: "run.item_changed",
      payload: { run_id: "r9", item_id: "i1", title: null, status: "queued", reason_code: null },
    });
    expect(s.runItems.r9?.i1?.status).toBe("queued");
  });
});
