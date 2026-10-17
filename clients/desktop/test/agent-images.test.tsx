import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError } from "@zwiim/sdk";
import { AgentImageChip } from "@/attachments/AgentImageChip";
import { FileLinksContext, Markdown } from "@/components/Markdown";
import { HostProvider } from "@/lib/host-context";
import { FakeTransport, tick } from "./fakes";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const workspace = {
  workspace_id: "w1",
  name: "a9b70af5",
  alias: null,
  kind: "worktree",
  parent_workspace_id: "root",
  is_git_repo: true,
  branch: "zwiim/a9b70af5",
  base_branch: "main",
  base_commit: null,
  removed: false,
  display_path: null,
} as const;
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

async function mount(ui: React.ReactNode, files: Record<string, Uint8Array> = {}) {
  const t = new FakeTransport();
  t.handlers["files.download"] = (p) => {
    const f = files[p.path];
    if (!f) throw new ZwiimClientError("not_found", "File not found", 404);
    return { name: p.path.split("/").pop(), mime_type: "image/png", size: f.length, offset: 0, data: b64(f) };
  };
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const links = { workspace, open: vi.fn(), openDiff: vi.fn() };
  URL.createObjectURL = vi.fn(() => "blob:thumb");
  URL.revokeObjectURL = vi.fn();
  render(
    <HostProvider value={controller}>
      <FileLinksContext.Provider value={links}>{ui}</FileLinksContext.Provider>
    </HostProvider>,
  );
  await act(tick);
  return { links, controller, t };
}

describe("agent image chip", () => {
  it("shows the file's icon and name, fetches nothing, and opens the preview", async () => {
    const { links, controller, t } = await mount(<AgentImageChip path=".zwiim/images/ab12.png" />, {
      ".zwiim/images/ab12.png": PNG,
    });
    const chip = screen.getByTestId("agent-image");
    expect(chip.textContent).toContain("ab12.png");
    expect(chip.querySelector("img:not([aria-hidden])")).toBeNull();
    expect(t.calls.some((c) => c.op === "files.download")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Open ab12.png" }));
    expect(links.open).toHaveBeenCalledWith(".zwiim/images/ab12.png");
    controller.dispose();
  });
});

describe("markdown paths", () => {
  it("shows a workspace image as a chip without fetching it", async () => {
    const { controller, t } = await mount(<Markdown text={"Here:\n\n![screen](.zwiim/images/my%20shot.png)"} />, {
      ".zwiim/images/my shot.png": PNG,
    });
    expect(screen.getByTestId("agent-image").textContent).toContain("my shot.png");
    expect(t.calls.some((c) => c.op === "files.download")).toBe(false);
    controller.dispose();
  });

  it("keeps remote and out-of-workspace images as text", async () => {
    const { controller } = await mount(<Markdown text={"![a](https://x.test/a.png) ![b](/etc/b.png)"} />);
    expect(screen.queryByTestId("agent-image")).toBeNull();
    expect(screen.getByTestId("markdown").textContent).toBe("[image: a] [image: b]");
    controller.dispose();
  });

  it("renders a link to a workspace pdf as a file chip that opens the preview", async () => {
    const { links, controller } = await mount(
      <Markdown text={"See [the report](out/report.pdf) and [code](src/a.ts)."} />,
    );
    const chip = screen.getByTestId("file-link-chip");
    expect(chip.textContent).toContain("report.pdf");
    fireEvent.click(chip);
    expect(links.open).toHaveBeenCalledWith("out/report.pdf");
    // A text file keeps the plain link.
    expect(screen.getByRole("link", { name: "code" })).toBeTruthy();
    controller.dispose();
  });
});
