import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ATTACHMENT_GONE_MESSAGE,
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type HistoryTurn,
  type SessionSnapshot,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { EMPTY_PENDING, needsUpload, pendingReducer, type PendingState } from "@/attachments/pending";
import { UPLOAD_FAILED_MESSAGE } from "@/attachments/usePendingAttachments";
import { FileLinksContext, type FileLinks } from "@/components/Markdown";
import { HostProvider } from "@/lib/host-context";
import { FileView, previewOf } from "@/panels/FilesPanel";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, hostHello, tick } from "./fakes";

const opened = vi.hoisted(() => vi.fn());
vi.mock("@/lib/bridge", async (original) => ({
  ...(await original<typeof import("@/lib/bridge")>()),
  openDownload: opened,
}));

afterEach(() => {
  cleanup();
  opened.mockReset();
});

const MB = 1024 * 1024;
const blob = new Blob(["x"]);
const entry = (id: string, size = 10) => ({ id, name: `${id}.pdf`, size, mime_type: "application/pdf", file: blob });

describe("pending attachments", () => {
  it("refuses files over 10 MB and past ten per message, saying why", () => {
    let s: PendingState = pendingReducer(EMPTY_PENDING, {
      type: "add",
      files: [entry("a"), entry("big", 10 * MB + 1), entry("b", 10 * MB)],
    });
    expect(s.files.map((f) => f.id)).toEqual(["a", "b"]);
    expect(s.errors).toEqual(["big.pdf is larger than 10 MB."]);
    s = pendingReducer(s, { type: "add", files: Array.from({ length: 9 }, (_, i) => entry(`n${i}`)) });
    expect(s.files).toHaveLength(10);
    expect(s.errors).toEqual(["Up to 10 files per message."]);
    s = pendingReducer(s, { type: "remove", id: "a" });
    expect(s.files).toHaveLength(9);
    expect(s.errors).toEqual([]);
  });

  it("uploads a file once per workspace, and again after the Host lost it", () => {
    let s = pendingReducer(EMPTY_PENDING, { type: "add", files: [entry("a"), entry("b")] });
    s = pendingReducer(s, { type: "uploaded", id: "a", attachment_id: "x1", workspace_id: "w1" });
    s = pendingReducer(s, { type: "failed", id: "b", error: "boom" });
    expect(needsUpload(s.files[0]!, "w1")).toBe(false);
    expect(needsUpload(s.files[0]!, "w2")).toBe(true);
    expect(needsUpload(s.files[1]!, "w1")).toBe(true);
    s = pendingReducer(s, { type: "forget_uploads" });
    expect(s.files.map((f) => [f.status, f.attachment_id])).toEqual([
      ["ready", undefined],
      ["failed", undefined],
    ]);
  });
});

const turn = (turn_id: string, role: string, blocks: HistoryTurn["blocks"]): HistoryTurn => ({
  turn_id,
  role,
  blocks,
  timestamp: "",
  completed_at: null,
  duration_ms: null,
  model: null,
  usage: null,
});

const snapshot = (extra: Partial<SessionSnapshot> = {}): SessionSnapshot => ({
  status: "connected",
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
  ...extra,
});

async function openSession(t: FakeTransport, links?: FileLinks) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const panel = <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} />;
  render(
    <HostProvider value={controller}>
      {links ? <FileLinksContext.Provider value={links}>{panel}</FileLinksContext.Provider> : panel}
    </HostProvider>,
  );
  await tick();
  await act(async () =>
    t.frame({
      subscription_id: "sub-1",
      session_id: "s1",
      type: "snapshot",
      epoch: "e",
      event_seq: 1,
      snapshot: snapshot(),
    } as never),
  );
  return controller;
}

const pdf = () => new File([new Uint8Array([37, 80, 68, 70])], "spec.pdf", { type: "application/pdf" });
const xlsx = () => new File([new Uint8Array([80, 75])], "data.xlsx", { type: "" });

function attach(...files: File[]) {
  fireEvent.change(screen.getByTestId("attachment-input"), { target: { files } });
}

describe("composer attachments", () => {
  it("uploads each file into the workspace, then sends their blocks with the text", async () => {
    const t = new FakeTransport();
    let n = 0;
    t.handlers["attachments.upload"] = (p) => ({ attachment_id: `id${++n}`, name: p.name, size: 4, mime_type: "x" });
    t.handlers["session.send"] = (p) => ({ session_id: p.session_id });
    const controller = await openSession(t);
    attach(pdf(), xlsx());
    expect(within(screen.getByTestId("pending-attachments")).getAllByTestId("pending-attachment")).toHaveLength(2);
    fireEvent.change(screen.getByPlaceholderText(/^Send a message/), { target: { value: "read these" } });
    fireEvent.click(screen.getByLabelText("Send"));
    await waitFor(() => expect(t.calls.some((c) => c.op === "session.send")).toBe(true));
    expect(t.calls.filter((c) => c.op === "attachments.upload").map((c) => c.params)).toEqual([
      { workspace_id: "w1", name: "spec.pdf", data: "JVBERg==" },
      { workspace_id: "w1", name: "data.xlsx", data: "UEs=" },
    ]);
    expect(t.calls.find((c) => c.op === "session.send")!.params.blocks).toEqual([
      { type: "text", text: "read these" },
      { type: "attachment", attachment_id: "id1" },
      { type: "attachment", attachment_id: "id2" },
    ]);
    await waitFor(() => expect(screen.queryByTestId("pending-attachments")).toBeNull());
    controller.dispose();
  });

  it("holds the message when a file does not upload; the next send retries only that file", async () => {
    const t = new FakeTransport();
    let fail = true;
    t.handlers["attachments.upload"] = (p) => {
      if (p.name === "data.xlsx" && fail) throw new ZwiimClientError("internal", "disk full", 500);
      return { attachment_id: p.name === "spec.pdf" ? "pdf1" : "xls1", name: p.name, size: 4, mime_type: "x" };
    };
    t.handlers["session.send"] = (p) => ({ session_id: p.session_id });
    const controller = await openSession(t);
    attach(pdf(), xlsx());
    // Files alone can be sent.
    fireEvent.click(screen.getByLabelText("Send"));
    await waitFor(() => expect(screen.getByText(UPLOAD_FAILED_MESSAGE)).toBeTruthy());
    expect(screen.getByText("Upload failed")).toBeTruthy();
    expect(t.calls.some((c) => c.op === "session.send")).toBe(false);

    fail = false;
    fireEvent.click(screen.getByLabelText("Send"));
    await waitFor(() => expect(t.calls.some((c) => c.op === "session.send")).toBe(true));
    expect(t.calls.filter((c) => c.op === "attachments.upload").map((c) => c.params.name)).toEqual([
      "spec.pdf",
      "data.xlsx",
      "data.xlsx",
    ]);
    expect(t.calls.find((c) => c.op === "session.send")!.params.blocks).toEqual([
      { type: "attachment", attachment_id: "pdf1" },
      { type: "attachment", attachment_id: "xls1" },
    ]);
    controller.dispose();
  });

  it("uploads again after the Host says an attachment is gone", async () => {
    const t = new FakeTransport();
    let gone = true;
    t.handlers["attachments.upload"] = (p) => ({ attachment_id: "a1", name: p.name, size: 4, mime_type: "x" });
    t.handlers["session.send"] = (p) => {
      if (gone) throw new ZwiimClientError("attachment_not_found", "attachment a1 not found", 404);
      return { session_id: p.session_id };
    };
    const controller = await openSession(t);
    attach(pdf());
    fireEvent.click(screen.getByLabelText("Send"));
    await waitFor(() => expect(screen.getByText(ATTACHMENT_GONE_MESSAGE)).toBeTruthy());
    gone = false;
    fireEvent.click(screen.getByLabelText("Send"));
    await waitFor(() => expect(t.calls.filter((c) => c.op === "session.send")).toHaveLength(2));
    expect(t.calls.filter((c) => c.op === "attachments.upload")).toHaveLength(2);
    controller.dispose();
  });

  it("offers no attaching against a Host without attachments", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "attachments") });
    const controller = await openSession(t);
    expect(screen.queryByLabelText("Attach files")).toBeNull();
    attach(pdf());
    expect(screen.queryByTestId("pending-attachments")).toBeNull();
    controller.dispose();
  });
});

describe("sent attachments", () => {
  it("shows a message's files, even with no text, and opens them beside the conversation", async () => {
    const t = new FakeTransport();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        turn("t1", "user", [
          {
            type: "attachment",
            attachment_id: "a1",
            name: "spec.pdf",
            mime_type: "application/pdf",
            size: 2_200_000,
            path: ".zwiim/attachments/a1/spec.pdf",
          },
          { type: "attachment", attachment_id: "a2", name: "old.csv", mime_type: "text/csv", path: ".zwiim/attachments/a2/old.csv" },
        ]),
      ],
    });
    const open = vi.fn();
    const controller = await openSession(t, { workspace: {} as WorkspaceInfo, open, openDiff: vi.fn() });
    const chips = screen.getAllByTestId("sent-attachment");
    expect(chips.map((c) => c.textContent)).toEqual(["spec.pdf2.1 MB", "old.csvremoved"]);
    fireEvent.click(chips[0]!);
    expect(open).toHaveBeenCalledWith(".zwiim/attachments/a1/spec.pdf");
    // A file no longer there is not a button.
    expect(chips[1]!.tagName).toBe("SPAN");
    controller.dispose();
  });
});

async function mountFile(t: FakeTransport, path: string) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <FileView workspaceId="w1" path={path} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

describe("file view", () => {
  it("sorts files into text, image and opened elsewhere", () => {
    expect(previewOf("a/notes.md")).toBe("text");
    expect(previewOf("data.csv")).toBe("text");
    expect(previewOf("page.html")).toBe("text");
    expect(previewOf("shot.PNG")).toBe("image");
    for (const p of ["spec.pdf", "data.xlsx", "old.xls", "memo.docx", "deck.pptx", "bundle.zip"]) {
      expect(previewOf(p)).toBe("none");
    }
  });

  it("opens a PDF in its own app instead of reading it as text", async () => {
    const t = new FakeTransport();
    t.handlers["files.download"] = () => ({ name: "spec.pdf", mime_type: "application/pdf", size: 4, data: "JVBERg==" });
    const controller = await mountFile(t, ".zwiim/attachments/a1/spec.pdf");
    expect(screen.getByTestId("file-no-preview").textContent).toContain("spec.pdf");
    expect(t.calls.some((c) => c.op === "files.read")).toBe(false);
    fireEvent.click(within(screen.getByTestId("file-no-preview")).getByRole("button", { name: /Open in default app/ }));
    await tick();
    expect(t.calls.find((c) => c.op === "files.download")!.params).toEqual({
      workspace_id: "w1",
      path: ".zwiim/attachments/a1/spec.pdf",
    });
    expect(opened).toHaveBeenCalledWith({ name: "spec.pdf", mime_type: "application/pdf", size: 4, data: "JVBERg==" });
    controller.dispose();
  });

  it("shows an image, and offers its own app when text cannot be shown", async () => {
    const t = new FakeTransport();
    t.handlers["files.download"] = () => ({ name: "shot.png", mime_type: "image/png", size: 4, data: "iVBORw==" });
    t.handlers["files.read"] = () => {
      throw new ZwiimClientError("invalid_request", "Binary files are not supported in preview", 400);
    };
    let controller = await mountFile(t, "shot.png");
    expect(screen.getByTestId("file-image").getAttribute("src")).toBe("data:image/png;base64,iVBORw==");
    controller.dispose();
    cleanup();

    controller = await mountFile(t, "model.bin");
    expect(screen.getByTestId("file-error").textContent).toBe("Binary files are not supported in preview");
    expect(screen.getAllByRole("button", { name: /Open in default app/ }).length).toBeGreaterThan(0);
    controller.dispose();
  });
});
