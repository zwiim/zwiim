import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as XLSX from "xlsx";
import { HostClient, HostController, ZwiimClientError } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { FileView } from "@/panels/FilesPanel";
import { FakeTransport, hostHello, tick } from "./fakes";

const pdf = vi.hoisted(() => ({
  openPdfSource: vi.fn(),
  renderPdf: vi.fn(),
  pdfDocumentParams: vi.fn(),
  PDF_INITIAL_BYTES: 1024 * 1024,
}));
vi.mock("@zwiim/preview/pdf", () => pdf);

const shell = vi.hoisted(() => ({ tauri: false }));
vi.mock("@/lib/bridge", async (o) => ({ ...(await o<typeof import("@/lib/bridge")>()), isTauri: () => shell.tauri }));
const host = vi.hoisted(() => ({ openPreview: vi.fn(), closePreview: vi.fn(), previewUrl: vi.fn() }));
vi.mock("@/lib/preview-host", () => host);

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
function serve(t: FakeTransport, files: Record<string, Uint8Array | Error>) {
  t.handlers["files.download"] = (p) => {
    const f = files[p.path];
    if (f instanceof Error) throw f;
    if (!f) throw new ZwiimClientError("not_found", "File not found", 404);
    const offset = p.offset ?? 0;
    const end = p.length === undefined ? f.length : Math.min(f.length, offset + p.length);
    return { name: p.path.split("/").pop(), mime_type: "application/octet-stream", size: f.length, offset, data: b64(f.subarray(offset, end)) };
  };
}
async function mount(t: FakeTransport, path: string) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(<HostProvider value={controller}><FileView workspaceId="w1" path={path} /></HostProvider>);
  await tick();
  return controller;
}

describe("previews", () => {
  it("renders a PDF with zoom controls", async () => {
    const handle = { zoomIn: vi.fn(), zoomOut: vi.fn(), fitWidth: vi.fn(), destroy: vi.fn() };
    pdf.openPdfSource.mockResolvedValue({ size: 3, initial: new Uint8Array(3), read: vi.fn() });
    pdf.renderPdf.mockResolvedValue(handle);
    const t = new FakeTransport();
    const controller = await mount(t, "memo.pdf");
    expect(pdf.openPdfSource).toHaveBeenCalledWith(controller.client, "w1", "memo.pdf");
    expect(pdf.renderPdf.mock.calls[0]![2]).toEqual({
      workerSrc: "/pdfjs/pdf.worker.min.mjs", cMapUrl: "/pdfjs/cmaps/", standardFontDataUrl: "/pdfjs/standard_fonts/", wasmUrl: "/pdfjs/wasm/",
    });
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    fireEvent.click(screen.getByRole("button", { name: "Fit width" }));
    expect([handle.zoomIn.mock.calls.length, handle.fitWidth.mock.calls.length]).toEqual([1, 1]);
    controller.dispose();
    cleanup();
    expect(handle.destroy).toHaveBeenCalled();
  });

  it("shows why a PDF cannot be previewed, keeping Open in default app", async () => {
    pdf.openPdfSource.mockRejectedValue(new ZwiimClientError("file_too_large_for_remote", "big", 413, { size: 40 * 1024 * 1024 }));
    const controller = await mount(new FakeTransport(), "big.pdf");
    expect(screen.getByTestId("file-error").textContent).toBe(
      "Too large to preview over a remote connection (40 MB). It opens on the same network.",
    );
    expect(screen.getAllByRole("button", { name: /Open in default app/ }).length).toBeGreaterThan(0);
    controller.dispose();
  });

  it("shows each sheet as a tab with the cap note", async () => {
    const wb = XLSX.utils.book_new();
    XLSX.utils.book_append_sheet(wb, XLSX.utils.aoa_to_sheet([["a", "<b>x</b>"]]), "One");
    XLSX.utils.book_append_sheet(wb, XLSX.utils.aoa_to_sheet(Array.from({ length: 1002 }, (_, i) => [i])), "Two");
    const t = new FakeTransport();
    serve(t, { "budget.xlsx": new Uint8Array(XLSX.write(wb, { type: "array", bookType: "xlsx" })) });
    const controller = await mount(t, "budget.xlsx");
    const table = screen.getByTestId("sheet-table");
    expect(within(table).getByText("<b>x</b>").tagName).toBe("TD");
    expect(screen.queryByText("Showing first 1,000 rows × 100 columns")).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Two" }));
    expect(screen.getByText("Showing first 1,000 rows × 100 columns")).toBeTruthy();
    controller.dispose();
  });

  it("says when the file is gone after Reload", async () => {
    const t = new FakeTransport();
    serve(t, { "data.csv": new TextEncoder().encode("a,b\n") });
    const controller = await mount(t, "data.csv");
    expect(screen.getByTestId("sheet-table")).toBeTruthy();
    serve(t, {});
    fireEvent.click(screen.getByTitle("Reload"));
    await tick();
    expect(screen.getByTestId("file-error").textContent).toBe("This file no longer exists.");
    expect(screen.getAllByRole("button", { name: /Open in default app/ }).length).toBeGreaterThan(0);
    controller.dispose();
  });

  it("reads the whole file once from an older Host, and says when it is too large", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ capabilities: hostHello().capabilities!.filter((c) => c !== "file_ranges") });
    t.handlers["files.download"] = () => {
      throw new ZwiimClientError("file_too_large", "The file is larger than 10 MB", 413);
    };
    const controller = await mount(t, "big.xlsx");
    expect(t.calls.filter((c) => c.op === "files.download").map((c) => c.params)).toEqual([{ workspace_id: "w1", path: "big.xlsx" }]);
    expect(screen.getByTestId("file-error").textContent).toBe("Too large to preview.");
    controller.dispose();
  });

  it("renders Word without scripts and opens its links in the browser", async () => {
    const word = await import("@zwiim/preview/word");
    vi.spyOn(word, "renderWord").mockResolvedValue(`<h1>Plan</h1><a href="https://example.com">site</a>`);
    const t = new FakeTransport();
    serve(t, { "spec.docx": new Uint8Array([1]) });
    const controller = await mount(t, "spec.docx");
    expect(screen.getByRole("heading", { name: "Plan" })).toBeTruthy();
    const bridge = await import("@/lib/bridge");
    const open = vi.spyOn(bridge, "openLink").mockResolvedValue();
    fireEvent.click(screen.getByText("site"));
    expect(open).toHaveBeenCalledWith("https://example.com");
    controller.dispose();
  });

  it("does not reload a preview when the workspace changes", async () => {
    const t = new FakeTransport();
    serve(t, { "data.csv": new TextEncoder().encode("a\n") });
    const controller = await mount(t, "data.csv");
    const reads = t.calls.filter((c) => c.op === "files.download").length;
    await t.feedEvent("workspace.diff_changed", { workspace_id: "w1" });
    expect(t.calls.filter((c) => c.op === "files.download").length).toBe(reads);
    controller.dispose();
  });
});

describe("html preview", () => {
  beforeEach(() => {
    shell.tauri = true;
  });
  afterEach(() => {
    shell.tauri = false;
  });

  it("frames the page at its own origin, sandboxed, with Rendered / Source", async () => {
    host.openPreview.mockResolvedValue("0123456789abcdef0123456789abcdef");
    host.previewUrl.mockImplementation((id: string, p: string) => `zwiim-preview://${id}/${p}`);
    const t = new FakeTransport();
    t.handlers["files.read"] = () => ({ content: "<h1>hi</h1>", path: "docs/report.html" });
    const controller = await mount(t, "docs/report.html");
    const frame = screen.getByTestId("html-frame") as HTMLIFrameElement;
    expect(frame.getAttribute("src")).toBe("zwiim-preview://0123456789abcdef0123456789abcdef/docs/report.html");
    expect(frame.getAttribute("sandbox")!.split(" ")).not.toContain("allow-top-navigation");
    expect(frame.getAttribute("sandbox")!.split(" ")).toContain("allow-scripts");
    fireEvent.click(screen.getByRole("radio", { name: "Source" }));
    await tick();
    expect(screen.getByText("<h1>hi</h1>")).toBeTruthy();
    cleanup();
    expect(host.closePreview).toHaveBeenCalledWith("0123456789abcdef0123456789abcdef");
    controller.dispose();
  });

  it("reloads the frame only from Reload", async () => {
    host.openPreview.mockResolvedValue("0123456789abcdef0123456789abcdef");
    host.previewUrl.mockReturnValue("zwiim-preview://x/a.html");
    const controller = await mount(new FakeTransport(), "a.html");
    const first = screen.getByTestId("html-frame");
    fireEvent.click(screen.getByTitle("Reload"));
    await tick();
    expect(screen.getByTestId("html-frame")).not.toBe(first);
    controller.dispose();
  });
});

describe("editing csv and tsv", () => {
  const enc = (text: string) => new TextEncoder().encode(text);
  function editable(t: FakeTransport, path: string, text: string) {
    serve(t, { [path]: enc(text) });
    t.handlers["files.read"] = () => ({ path, content: text, etag: "e1", mtime_ms: null, readonly: false, line_ending: "lf" });
    t.handlers["files.write"] = () => ({ path, etag: "e2", mtime_ms: null, readonly: false, line_ending: "lf" });
  }
  async function edit() {
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await tick();
  }
  function setCell(text: string, value: string) {
    fireEvent.click(within(screen.getByTestId("sheet-editor")).getByText(text));
    const box = screen.getByRole("textbox", { name: "Cell" });
    fireEvent.change(box, { target: { value } });
    fireEvent.keyDown(box, { key: "Enter" });
  }
  const writes = (t: FakeTransport) => t.calls.filter((c) => c.op === "files.write").map((c) => c.params);

  it("saves an edited cell, rewriting only its row, then shows the saved file", async () => {
    const t = new FakeTransport();
    editable(t, "data.csv", '"name" ,qty\r\npens,2\r\ncups,3\r\n');
    const controller = await mount(t, "data.csv");
    await edit();
    expect(screen.getByRole("button", { name: "Save" }).hasAttribute("disabled")).toBe(true);
    setCell("2", "say \"hi\", 9");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await tick();
    expect(writes(t)).toEqual([
      { workspace_id: "w1", path: "data.csv", content: '"name" ,qty\r\npens,"say ""hi"", 9"\r\ncups,3\r\n', etag: "e1" },
    ]);
    expect(screen.queryByTestId("sheet-editor")).toBeNull();
    expect(screen.getByTestId("sheet-table")).toBeTruthy();
    expect(t.calls.filter((c) => c.op === "files.download").length).toBe(2);
    controller.dispose();
  });

  it("adds and deletes rows in a tsv", async () => {
    const t = new FakeTransport();
    editable(t, "list.tsv", "a\tb\n1\t2\n3\t4\n");
    const controller = await mount(t, "list.tsv");
    await edit();
    fireEvent.click(screen.getByRole("button", { name: "Delete row 2" }));
    fireEvent.click(screen.getByRole("button", { name: "Add row" }));
    const box = screen.getByRole("textbox", { name: "Cell" });
    fireEvent.change(box, { target: { value: "x,y" } });
    fireEvent.keyDown(box, { key: "Tab" });
    fireEvent.change(screen.getByRole("textbox", { name: "Cell" }), { target: { value: "z" } });
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Cell" }), { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await tick();
    expect(writes(t)[0]!.content).toBe("a\tb\n3\t4\nx,y\tz\n");
    controller.dispose();
  });

  it("refuses to overwrite a file that changed, keeping the edits until Reload", async () => {
    const t = new FakeTransport();
    editable(t, "data.csv", "a,b\n1,2\n");
    t.handlers["files.write"] = () => {
      throw new ZwiimClientError("file_changed", "File has changed on disk. Reload the file before saving.", 409);
    };
    const controller = await mount(t, "data.csv");
    await edit();
    setCell("2", "7");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await tick();
    expect(screen.getByRole("alert").textContent).toContain("This file changed on disk");
    expect(within(screen.getByTestId("sheet-editor")).getByText("7")).toBeTruthy();
    fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Reload" }));
    await tick();
    expect(screen.queryByTestId("sheet-editor")).toBeNull();
    expect(within(screen.getByTestId("sheet-table")).getByText("2")).toBeTruthy();
    controller.dispose();
  });

  it("cancels without saving", async () => {
    const t = new FakeTransport();
    editable(t, "data.csv", "a,b\n");
    const controller = await mount(t, "data.csv");
    await edit();
    setCell("b", "c");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByTestId("sheet-editor")).toBeNull();
    expect(within(screen.getByTestId("sheet-table")).getByText("b")).toBeTruthy();
    expect(writes(t)).toEqual([]);
    controller.dispose();
  });

  it("offers Edit only for csv and tsv that fit, on a Host that can save", async () => {
    const t = new FakeTransport();
    editable(t, "long.csv", "x\n".repeat(1001));
    let controller = await mount(t, "long.csv");
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    controller.dispose();
    cleanup();

    const old = new FakeTransport();
    old.handlers["host.hello"] = () => hostHello({ capabilities: hostHello().capabilities!.filter((c) => c !== "files.write") });
    editable(old, "data.csv", "a\n");
    controller = await mount(old, "data.csv");
    expect(screen.getByTestId("sheet-table")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    controller.dispose();
    cleanup();

    const wb = XLSX.utils.book_new();
    XLSX.utils.book_append_sheet(wb, XLSX.utils.aoa_to_sheet([["a"]]), "One");
    const x = new FakeTransport();
    serve(x, { "b.xlsx": new Uint8Array(XLSX.write(wb, { type: "array", bookType: "xlsx" })) });
    controller = await mount(x, "b.xlsx");
    expect(screen.getByTestId("sheet-table")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    controller.dispose();
  });
});
