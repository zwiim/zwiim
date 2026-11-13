import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HostClient, HostController } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { HostDownloads } from "@/panels/HostDownloads";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

describe("host downloads", () => {
  it("shows each download with its progress until it ends", async () => {
    const t = new FakeTransport();
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <HostDownloads />
      </HostProvider>,
    );
    expect(screen.queryByRole("status")).toBeNull();

    await t.feedEvent("download.changed", {
      download: { download_id: "d1", label: "Claude Code", received: 23_000_000, total: 89_000_000 },
    });
    await t.feedEvent("download.changed", {
      download: { download_id: "d2", label: "Browser engine 157", received: 4_200_000, total: null },
    });
    const rows = screen.getByRole("status", { name: "Downloads" }).children;
    expect([...rows].map((r) => r.textContent)).toEqual([
      "Downloading Claude Code…23 MB / 89 MB",
      "Downloading Browser engine 157…4.2 MB",
    ]);

    await t.feedEvent("download.removed", { download_id: "d1" });
    await t.feedEvent("download.removed", { download_id: "d2" });
    expect(screen.queryByRole("status")).toBeNull();
  });
});
