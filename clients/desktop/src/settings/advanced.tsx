// The Advanced tab: how much terminal output the host keeps, whether a plain
// `git push` from a worktree sets its upstream, and where the host keeps
// worktrees and repositories. Agents run through the host, so there is no binary to point at here.

import { useCallback, useEffect, useState } from "react";
import { DEFAULT_SCROLLBACK_BYTES, type HostFolders, type HostSettings } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { ControlSelect } from "@/panels/OptionsBar";
import { SettingRow, ToggleRow } from "@/settings/rows";

type Values = HostSettings["values"];

/** The select's value for the host's default; the host keeps it as unset. */
const DEFAULT = "";

const MIB = 1024 * 1024;

/** The amounts offered, beside the default. */
const AMOUNTS = [256 * 1024, 4 * MIB, 16 * MIB];

export function AdvancedSettings({ bridge, remote }: { bridge: NativeBridge; remote: boolean }) {
  const controller = useController();
  const online = useOnline();
  const [values, setValues] = useState<Values | null>(null);
  const [folders, setFolders] = useState<HostFolders | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("settings.get", {}).then(
      (s) => setValues(s.values),
      (e) => setError(errorMessage(e)),
    );
    controller.client.request("host.folders", {}).then(setFolders, (e) => setError(errorMessage(e)));
  }, [controller]);

  const update = useCallback(
    (patch: Values) => {
      setError(null);
      controller.client
        .request("settings.update", { values: patch })
        .then((s) => setValues(s.values))
        .catch((e) => setError(errorMessage(e)));
    },
    [controller],
  );

  // Showing a folder needs this computer's file manager, so only for a host on it.
  const reveal = remote ? undefined : bridge.folders;
  const show = (path: string) => {
    setError(null);
    reveal?.reveal(path).catch((e) => setError(errorMessage(e)));
  };

  if (!values) {
    return error ? (
      <p className="text-destructive" role="alert">
        {error}
      </p>
    ) : null;
  }
  const kept = values["terminal.scrollback"];
  // An amount set elsewhere stays shown as chosen.
  const amounts =
    typeof kept === "number" && !AMOUNTS.includes(kept) ? [...AMOUNTS, kept].sort((a, b) => a - b) : AMOUNTS;

  return (
    <>
      <p className="text-muted-foreground">
        How the host runs terminals and Git for every device. Agents run through the host, so they need no setup here.
      </p>
      <SettingRow
        setting={{
          id: "terminal-scrollback",
          title: "Terminal output to keep",
          description: "Per terminal. What a reopened terminal or another device scrolls back through.",
        }}
      >
        <ControlSelect
          label="Terminal output to keep"
          value={typeof kept === "number" ? String(kept) : DEFAULT}
          disabled={!online}
          onChange={(v) => update({ "terminal.scrollback": v === DEFAULT ? null : Number(v) })}
          options={[
            { value: DEFAULT, label: `${size(DEFAULT_SCROLLBACK_BYTES)} (default)` },
            ...amounts.map((n) => ({ value: String(n), label: size(n) })),
          ]}
        />
      </SettingRow>
      <ToggleRow
        setting={{
          id: "push-sets-upstream",
          title: "Set upstream on plain git push",
          description:
            "A worktree's first git push, from a terminal, script or agent, creates its branch on the remote and tracks it.",
        }}
        testId="push-sets-upstream"
        checked={values["git.push_sets_upstream"] !== false}
        disabled={!online}
        onChange={(on) => update({ "git.push_sets_upstream": on ? null : false })}
      />
      {folders && (
        <section className="flex flex-col gap-2" data-setting-id="host-folders">
          <span className="text-ui">Folders</span>
          {(
            [
              ["Worktrees", folders.worktrees],
              ["Repositories", folders.repositories],
            ] as const
          ).map(([label, path]) => (
            <div key={label} className="flex items-center gap-3 border-b pb-2">
              <span className="text-muted-foreground w-24 shrink-0">{label}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-xs" title={path}>
                {path}
              </span>
              {reveal && (
                <Button size="xs" variant="outline" onClick={() => show(path)}>
                  Show
                </Button>
              )}
            </div>
          ))}
        </section>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

function size(bytes: number): string {
  return bytes >= MIB ? `${+(bytes / MIB).toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`;
}
