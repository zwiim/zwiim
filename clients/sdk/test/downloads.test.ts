import { describe, expect, it } from "vitest";
import { downloadProgress, formatBytes } from "../src/state/downloads";
import { initialHostState, reduceHost } from "../src/state/store";

const d = (download_id: string, received: number, total: number | null = null) => ({
  download_id,
  label: "Claude Code",
  received,
  total,
});

describe("downloads", () => {
  it("reads progress as the Mac counts bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1_500)).toBe("1.5 KB");
    expect(formatBytes(23_400_000)).toBe("23 MB");
    expect(formatBytes(2_340_000)).toBe("2.3 MB");
    expect(formatBytes(1_200_000_000)).toBe("1.2 GB");
    expect(downloadProgress({ received: 23_000_000, total: 89_000_000 })).toBe("23 MB / 89 MB");
    expect(downloadProgress({ received: 23_000_000, total: null })).toBe("23 MB");
  });

  it("follows the snapshot and the feed", () => {
    let s = reduceHost(initialHostState(), {
      type: "host_snapshot",
      snapshot: { epoch: "e", seq: 0, workspaces: [], sessions: [], terminals: [], downloads: [d("a", 1)] },
    });
    expect(s.downloads).toEqual([d("a", 1)]);
    s = reduceHost(s, { type: "host_event", event: { seq: 1, type: "download.changed", payload: { download: d("b", 0, 9) } } });
    s = reduceHost(s, { type: "host_event", event: { seq: 2, type: "download.changed", payload: { download: d("a", 5) } } });
    expect(s.downloads).toEqual([d("a", 5), d("b", 0, 9)]);
    s = reduceHost(s, { type: "host_event", event: { seq: 3, type: "download.removed", payload: { download_id: "a" } } });
    expect(s.downloads).toEqual([d("b", 0, 9)]);
    s = reduceHost(s, {
      type: "host_snapshot",
      snapshot: { epoch: "e", seq: 3, workspaces: [], sessions: [], terminals: [] },
    });
    expect(s.downloads).toEqual([]);
  });
});
