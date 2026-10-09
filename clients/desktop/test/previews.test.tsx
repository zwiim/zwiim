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
