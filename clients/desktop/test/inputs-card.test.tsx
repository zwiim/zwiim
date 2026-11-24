import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  HostClient,
  HostController,
  ZwiimClientError,
  type PlaybookCheck,
  type PlaybookFact,
  type PlaybookFile,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { InputsCard } from "@/panels/automations/InputsCard";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

const resume: PlaybookFile = {
  input_name: "Resume",
  file: "resume.pdf",
  path: null,
  size: 240 * 1024,
  sha256: "a".repeat(64),
  added_at: "2026-11-22T10:00:00Z",
  used_by_steps: [2],
};

const email: PlaybookFact = { input_name: "Email", value: "ada@example.com", used_by_steps: [1] };

function transport(
  files: PlaybookFile[] = [resume],
  facts: PlaybookFact[] = [email],
  check: PlaybookCheck = { text: "Application sent", editable: true },
) {
  const t = new FakeTransport();
  t.handlers["playbook.inputs"] = () => ({ files, facts, check });
  t.handlers["playbook.check.set"] = (p) => ({ playbook: { playbook_id: p.playbook_id } });
  t.handlers["playbook.file.set"] = (p) => ({
    playbook: { playbook_id: p.playbook_id },
    file: { input_name: p.input_name, file: "new.pdf", size: 1, sha256: "b".repeat(64) },
  });
  t.handlers["playbook.file.remove"] = () => {
    throw new ZwiimClientError("input_in_use", "A step still uses this file.", 409);
  };
  t.handlers["playbook.input.set_fact"] = (p) => ({ playbook: { playbook_id: p.playbook_id } });
  return t;
}

async function mount(t: FakeTransport, picked: string | null = "/Users/ada/new.pdf") {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <InputsCard playbookId="pb1" pickFile={() => Promise.resolve(picked)} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

function sent(t: FakeTransport, op: string) {
  return t.calls.filter((m) => m.op === op).map((m) => m.params);
}

describe("inputs card", () => {
  it("lists a stored file with its size and date, and a fact", async () => {
    const t = transport();
    await mount(t);
    expect(screen.getByTestId("input-file-Resume").textContent).toContain("resume.pdf · 240 KB · added 22 Nov");
    expect(screen.getByTestId("input-fact-Email").textContent).toContain("ada@example.com");
  });

  it("replaces a file with the one picked on this computer", async () => {
    const t = transport();
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Replace Resume" }));
    await tick();
    expect(sent(t, "playbook.file.set")).toEqual([
      { playbook_id: "pb1", input_name: "Resume", path: "/Users/ada/new.pdf" },
    ]);
  });

  it("does nothing when the picker is cancelled", async () => {
    const t = transport();
    await mount(t, null);
    fireEvent.click(screen.getByRole("button", { name: "Replace Resume" }));
    await tick();
    expect(sent(t, "playbook.file.set")).toEqual([]);
  });

  it("says why a file a step uses cannot be removed", async () => {
    const t = transport();
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Remove Resume" }));
    await tick();
    expect(screen.getByRole("alert").textContent).toBe(
      "A step still uses this file. Remove that step or replace the file.",
    );
  });

  it("an older file on this computer offers to keep a copy", async () => {
    const t = transport([{ ...resume, file: null, path: "~/old/resume.pdf", size: null, sha256: null, added_at: null }]);
    await mount(t);
    expect(screen.getByTestId("input-file-Resume").textContent).toContain("On this computer: ~/old/resume.pdf");
  });

  it("edits a fact in place", async () => {
    const t = transport();
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "Edit Email" }));
    const field = screen.getByRole("textbox", { name: "Email" });
    fireEvent.change(field, { target: { value: "new@example.com" } });
    fireEvent.submit(field.closest("form")!);
    await tick();
    expect(sent(t, "playbook.input.set_fact")).toEqual([
      { playbook_id: "pb1", input_name: "Email", value: "new@example.com" },
    ]);
  });

  it("changes the text that shows it worked, and clears it for the agent to judge", async () => {
    const t = transport();
    await mount(t);
    expect(screen.getByTestId("input-check").textContent).toContain("Application sent");
    fireEvent.click(screen.getByRole("button", { name: "Edit Shows it worked" }));
    const field = screen.getByRole("textbox", { name: "Shows it worked" });
    fireEvent.change(field, { target: { value: "Order placed" } });
    fireEvent.submit(field.closest("form")!);
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Edit Shows it worked" }));
    const again = screen.getByRole("textbox", { name: "Shows it worked" });
    fireEvent.change(again, { target: { value: "  " } });
    fireEvent.submit(again.closest("form")!);
    await tick();
    expect(sent(t, "playbook.check.set")).toEqual([
      { playbook_id: "pb1", text: "Order placed" },
      { playbook_id: "pb1", text: null },
    ]);
  });

  it("with no text the agent judges, and checks kept in the text are not edited here", async () => {
    await mount(transport([resume], [email], { text: null, editable: true }));
    expect(screen.getByTestId("input-check").textContent).toContain("The agent judges from the page");
    cleanup();
    await mount(transport([resume], [email], { text: "Sent", editable: false }));
    expect(screen.getByTestId("input-check").textContent).toContain("Sent");
    expect(screen.queryByRole("button", { name: "Edit Shows it worked" })).toBeNull();
  });

  it("adds a file named after it", async () => {
    const t = transport([], []);
    await mount(t, "/Users/ada/Cover letter.pdf");
    fireEvent.click(screen.getByRole("button", { name: "Add file" }));
    await tick();
    expect(sent(t, "playbook.file.set")).toEqual([
      { playbook_id: "pb1", input_name: "Cover letter", path: "/Users/ada/Cover letter.pdf" },
    ]);
  });
});
