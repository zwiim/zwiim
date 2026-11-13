// What the Host is downloading (an agent's adapter, the browser engine, a
// Host update), one row each above the composer: "Downloading Claude Code…
// 23 MB / 89 MB". Rows go when their download ends.

import { Download } from "lucide-react";
import { downloadProgress } from "@zwiim/sdk";
import { useHost } from "@/lib/host-context";

export function HostDownloads() {
  const downloads = useHost((s) => s.downloads);
  if (downloads.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5" role="status" aria-label="Downloads">
      {downloads.map((d) => (
        <div
          key={d.download_id}
          className="border-border bg-card animate-fade-in flex h-10 items-center gap-2.5 rounded-xl border px-3.5"
        >
          <Download className="text-muted-foreground size-4 shrink-0" aria-hidden />
          <span className="text-ui min-w-0 truncate font-medium">Downloading {d.label}…</span>
          <span className="text-muted-foreground text-ui shrink-0 tabular-nums">{downloadProgress(d)}</span>
        </div>
      ))}
    </div>
  );
}
