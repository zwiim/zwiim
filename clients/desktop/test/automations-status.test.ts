import { describe, expect, it } from "vitest";
import type { ItemStatus, RunItemView } from "@zwiim/sdk";
import {
  defaultOpen,
  failureText,
  itemStatus,
  needsYouText,
  runCounts,
  sectionLine,
} from "@/panels/automations/status";

function item(
  status: ItemStatus,
  extra: Partial<RunItemView> = {},
): RunItemView {
  return {
    run_id: "r1",
    item_id: `i-${status}`,
    title: null,
    status,
    reason_code: null,
    ...extra,
  };
}

const zero = { total: 0, sent: 0, need: 0 };

describe("itemStatus", () => {
  it.each([
    ["queued", "idle", "Queued", null],
    ["working", "work", "Working", null],
    ["waiting_approval", "need", "Needs your OK", "Needs your OK"],
    ["done", "done", "Sent", null],
    ["skipped", "off", "Skipped by your rules", null],
    ["deferred", "off", "Not reached today", null],
    ["failed", "err", "Failed", null],
    ["retries_exhausted", "err", "Gave up", null],
    ["stopped", "off", "Stopped", null],
  ] as const)("%s", (status, dot, label, overlay) => {
    expect(itemStatus(item(status))).toEqual({ dot, label, overlay });
  });

  it("a question shows what it asks", () => {
    expect(
      itemStatus(item("waiting_person", { question: "Expected salary?" })),
    ).toEqual({
      dot: "need",
      label: "Question",
      overlay: "Question for you · Expected salary?",
    });
  });

  it("waiting on the person without a question says it needs them", () => {
    expect(
      itemStatus(item("waiting_person", { reason_code: "check_failed" })),
    ).toEqual({
      dot: "need",
      label: "Needs you",
      overlay: "Needs you",
    });
  });

  it("a skip says who skipped it", () => {
    expect(
      itemStatus(item("skipped", { reason_code: "approval_rejected" })).label,
    ).toBe("Skipped by you");
    expect(
      itemStatus(item("skipped", { reason_code: "already_done" })).label,
    ).toBe("Already sent");
  });
});

describe("runCounts", () => {
  it("counts sent and needs-you", () => {
    const items = [
      item("done"),
      item("done"),
      item("waiting_approval"),
      item("waiting_person"),
      item("queued"),
    ];
    expect(runCounts(items)).toEqual({ total: 5, sent: 2, need: 2 });
  });
});

describe("sectionLine", () => {
  it.each([
    ["running", "Running · 1 of 4 sent"],
    ["discovering", "Running · 1 of 4 sent"],
    ["paused", "Paused"],
    ["stopped", "Stopped"],
    ["failed", "Failed"],
    ["done", "1 sent"],
  ] as const)("%s", (status, text) => {
    expect(sectionLine({ status }, { total: 4, sent: 1, need: 0 })).toEqual({
      text,
      need: false,
    });
  });

  it("a playbook never run", () => {
    expect(sectionLine(null, zero)).toEqual({
      text: "Not run yet",
      need: false,
    });
  });

  it("a done run that found nothing new", () => {
    expect(
      sectionLine({ status: "done", outcome: "nothing_new" }, zero),
    ).toEqual({ text: "Nothing new", need: false });
  });

  it("needs-you wins", () => {
    expect(
      sectionLine({ status: "running" }, { total: 4, sent: 1, need: 2 }),
    ).toEqual({ text: "2 need you", need: true });
  });
});

describe("defaultOpen", () => {
  it("opens active runs and runs that need the person", () => {
    expect(defaultOpen({ status: "paused" }, zero)).toBe(true);
    expect(defaultOpen({ status: "running" }, zero)).toBe(true);
    expect(
      defaultOpen({ status: "done" }, { total: 2, sent: 2, need: 0 }),
    ).toBe(false);
    expect(
      defaultOpen({ status: "done" }, { total: 2, sent: 1, need: 1 }),
    ).toBe(true);
    expect(defaultOpen(null, zero)).toBe(false);
  });
});

describe("failureText", () => {
  it("says why a run failed in plain words", () => {
    expect(failureText("agent_unavailable")).toBe("The agent could not start");
    expect(failureText("playbook_invalid")).toBe(
      "The playbook's text has a problem",
    );
    expect(failureText("something_new")).toBe(
      "Something went wrong on the host",
    );
    expect(failureText(null)).toBe("Something went wrong on the host");
  });
});

describe("needsYouText", () => {
  it("says why an item needs the person", () => {
    expect(needsYouText("check_failed")).toBe(
      "It could not confirm the send went through. Check the page on the site.",
    );
    expect(needsYouText("login_expired")).toBe(
      "The site signed it out. Sign in to the site again.",
    );
    expect(needsYouText(null)).toBe("The page needs you.");
  });
});
