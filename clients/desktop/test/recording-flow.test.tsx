import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type PlaybookDraft } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { RecordingBar, RecordingProvider, useRecording } from "@/panels/automations/recording";
import { ReviewRecording } from "@/panels/automations/ReviewRecording";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

function draft(extra: Partial<PlaybookDraft> = {}): PlaybookDraft {
  return {
    name: "Apply on jobs.test",
    goal: "Do what I did on jobs.test: Submit application.",
    source: { type: "search", url: "https://jobs.test/search" },
    sites: ["jobs.test", "cdn.test"],
    steps: [
      { action: "click", target: "Apply", role: "button", url: "https://jobs.test/j/1", value: { kind: "none" } },
      {
        action: "fill",
        target: "Email",
        role: "textbox",
        url: "https://jobs.test/j/1",
        value: { kind: "my_info", input: "Email", value: "ada@example.com" },
      },
      {
        action: "file",
        target: "CV",
        role: "button",
        url: "https://jobs.test/j/1",
        value: { kind: "file", input: "CV", path: null },
      },
      {
        action: "click",
        target: "Submit application",
        role: "button",
        url: "https://jobs.test/j/1",
        value: { kind: "none" },
      },
    ],
    send_step: 3,
    send_kind: "submit_application",
    check_text: null,
    ...extra,
  };
}

function transport() {
  const t = new FakeTransport();
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
  t.handlers["browser.record.stop"] = () => ({
    recording: { tab_id: "mine1", start_url: "https://jobs.test/search", steps: [] },
    draft: draft(),
  });
  t.handlers["playbook.create_from_draft"] = () => ({
    playbook: {
      playbook_id: "pb9",
      name: "Apply on jobs.test",
      version: 1,
      show_me: false,
      freeze_learning: false,
      sites: [{ site: "jobs.test", risk_accepted: true }],
    },
  });
  t.handlers["run.try"] = () => ({ run_id: "r9" });
  return t;
}

async function controllerFor(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  return controller;
}

function calls(t: FakeTransport, op: string) {
  return t.calls.filter((c) => c.op === op).map((c) => c.params);
}

/** Shows what the recording context holds, and starts one. */
function Probe() {
  const r = useRecording()!;
  return (
    <div>
      <span data-testid="recording-tab">{r.tabId ?? "none"}</span>
      <span data-testid="review">{r.review ? r.review.draft.name : "none"}</span>
      <button onClick={() => void r.record()}>Record</button>
      {r.tabId && <RecordingBar tabId={r.tabId} />}
    </div>
  );
}

describe("recording flow", () => {
  it("records in a new tab of one's own, shows its steps, and stops into a review", async () => {
    const t = transport();
    const controller = await controllerFor(t);
    const showBrowser = vi.fn();
    const review = vi.fn();
    render(
      <HostProvider value={controller}>
        <RecordingProvider canShowBrowser onShowBrowser={showBrowser} onReview={review}>
          <Probe />
        </RecordingProvider>
      </HostProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    await tick();
    expect(calls(t, "browser.tab.open")).toEqual([{ owner: { kind: "personal" } }]);
    expect(calls(t, "browser.record.start")).toEqual([{ tab_id: "mine1" }]);
    expect(showBrowser).toHaveBeenCalled();
    expect(screen.getByTestId("recording-tab").textContent).toBe("mine1");
    expect(screen.getByTestId("recording-bar").textContent).toContain("Recording");

    controller.watchBrowser();
    await tick();
    await act(async () => {
      t.frame({ type: "browser.snapshot", tabs: [], engine: { state: "running", version: "157" } } as never);
      t.frame({
        type: "browser.tab.recorded",
        tab_id: "mine1",
        owner: { kind: "personal" },
        count: 3,
        last: { action: "click", name: "Apply" },
      } as never);
    });
    expect(screen.getByTestId("recording-bar").textContent).toContain("3 steps");
    expect(screen.getByTestId("recording-bar").textContent).toContain("Apply");

    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await tick();
    expect(calls(t, "browser.record.stop")).toEqual([{ tab_id: "mine1" }]);
    expect(screen.getByTestId("review").textContent).toBe("Apply on jobs.test");
    expect(screen.getByTestId("recording-tab").textContent).toBe("none");
    expect(review).toHaveBeenCalled();
  });

  it("cancel drops the recording", async () => {
    const t = transport();
    const controller = await controllerFor(t);
    render(
      <HostProvider value={controller}>
        <RecordingProvider canShowBrowser onShowBrowser={() => {}} onReview={() => {}}>
          <Probe />
        </RecordingProvider>
      </HostProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await tick();
    expect(calls(t, "browser.record.stop")).toHaveLength(1);
    expect(screen.getByTestId("review").textContent).toBe("none");
    expect(screen.getByTestId("recording-tab").textContent).toBe("none");
  });

  it("without a workspace on screen, says where to record", async () => {
    const t = transport();
    const controller = await controllerFor(t);
    const Err = () => <p data-testid="err">{useRecording()!.error}</p>;
    render(
      <HostProvider value={controller}>
        <RecordingProvider canShowBrowser={false} onShowBrowser={() => {}} onReview={() => {}}>
          <Probe />
          <Err />
        </RecordingProvider>
      </HostProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    await tick();
    expect(calls(t, "browser.tab.open")).toEqual([]);
    expect(screen.getByTestId("err").textContent).toBe(
      "Add a project folder first: recording happens in the Browser panel beside it.",
    );
  });
});

describe("review", () => {
  async function mount(d: PlaybookDraft, picked: string | null = "/Users/ada/cv.pdf") {
    const t = transport();
    const controller = await controllerFor(t);
    const onSaved = vi.fn();
    render(
      <HostProvider value={controller}>
        <ReviewRecording draft={d} pickFile={() => Promise.resolve(picked)} onSaved={onSaved} onClose={() => {}} />
      </HostProvider>,
    );
    await tick();
    return { t, onSaved };
  }

  it("cannot save until a step sends and each file is picked", async () => {
    await mount(draft({ send_step: null }));
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Choose which step sends.")).toBeTruthy();
    fireEvent.click(screen.getByRole("radio", { name: "Submit application sends" }));
    expect(screen.getByText("Pick the file for CV.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Pick CV" }));
    await tick();
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("keeps a value as fixed text, drops a site, and saves then tries", async () => {
    const { t, onSaved } = await mount(draft());
    fireEvent.click(screen.getByRole("button", { name: "Pick CV" }));
    await tick();
    fireEvent.click(screen.getByRole("checkbox", { name: "Email is my info" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "cdn.test" }));
    fireEvent.change(screen.getByLabelText("Text that shows it worked"), { target: { value: "Application sent" } });
    fireEvent.click(screen.getByRole("button", { name: "Save and try on one item" }));
    await tick();
    const [{ draft: saved }] = calls(t, "playbook.create_from_draft") as [{ draft: PlaybookDraft }];
    expect(saved.steps[1]!.value).toEqual({ kind: "fixed", text: "ada@example.com" });
    expect(saved.steps[2]!.value).toEqual({ kind: "file", input: "CV", path: "/Users/ada/cv.pdf" });
    expect(saved.sites).toEqual(["jobs.test"]);
    expect(saved.check_text).toBe("Application sent");
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ playbook_id: "pb9" }), true);
  });
});
