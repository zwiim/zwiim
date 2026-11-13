// Where the host's browser saves what pages download (design, decision 13):
// by default each session's own folder in its worktree, removed with the
// session; or a root folder the person chooses, which every worktree follows
// as `<root>/zwiim/<repo>/downloads/`. Only for a host on this computer: the
// folder is one of its own.

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { SettingRow } from "@/settings/rows";

export function BrowserDownloadsSetting({ bridge }: { bridge: NativeBridge }) {
  const controller = useController();
  const online = useOnline();
  // undefined until read; a host without the browser never answers, and the row stays hidden.
  const [root, setRoot] = useState<string | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("browser.downloads.location", {}).then(
      (l) => setRoot(l.root),
      () => {},
    );
  }, [controller]);

  if (root === undefined) return null;

  const set = (next: string | null) => {
    setError(null);
    controller.client.request("browser.downloads.set_location", { root: next }).then(
      (l) => setRoot(l.root),
      (e) => setError(errorMessage(e)),
    );
  };
  const choose = () => {
    bridge.pickFolder().then(
      (picked) => picked && set(picked),
      (e) => setError(errorMessage(e)),
    );
  };

  return (
    <SettingRow
      setting={{
        id: "browser-downloads",
        title: "Browser downloads",
        description: root
          ? "Every worktree saves what pages download here, in zwiim/<repository>/downloads. Zwiim never deletes these files."
          : "Each session saves what pages download in its worktree, under .zwiim/browser, removed with the session. Your own tabs save to Downloads.",
      }}
      below={
        <>
          {root && (
            <div className="flex items-center gap-3">
              <span className="min-w-0 flex-1 truncate font-mono text-xs" title={root}>
                {root}
              </span>
              {bridge.folders && (
                <Button size="xs" variant="outline" onClick={() => bridge.folders?.reveal(root).catch((e) => setError(errorMessage(e)))}>
                  Show
                </Button>
              )}
            </div>
          )}
          {error && (
            <p className="text-destructive" role="alert">
              {error}
            </p>
          )}
        </>
      }
    >
      <div className="flex shrink-0 gap-2">
        {root && (
          <Button size="xs" variant="outline" disabled={!online} onClick={() => set(null)}>
            Use worktrees
          </Button>
        )}
        <Button size="xs" variant="outline" disabled={!online} onClick={choose}>
          {root ? "Change…" : "Choose folder…"}
        </Button>
      </div>
    </SettingRow>
  );
}
