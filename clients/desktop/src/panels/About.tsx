import { useCallback, useEffect, useState } from "react";
import type { Compatibility, HostDiagnostics, HostUpdateApply, HostUpdateStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { DESKTOP_CLIENT } from "@/lib/app-info";
import type { DesktopUpdateStatus, HostLoginStatus, NativeBridge } from "@/lib/bridge";
import { errorMessage, updateRequiredMessage, useController, useHost } from "@/lib/host-context";
import { Modal } from "@/panels/Hosts";

/** While the dialog is open, how often the host's update state is read. */
const POLL_MS = 3000;

const COMPAT: Record<Compatibility["status"], string> = {
  compatible: "Compatible",
  limited: "Compatible, some features unavailable",
  update_required: "Update required",
};

const UNSUPPORTED: Record<string, string> = {
  unsigned_build: "This host was not built by the release pipeline, so it does not update itself.",
  invalid_release_key: "This host's release key is invalid, so it cannot verify updates.",
  unsupported_platform: "Updates are not published for this host's platform.",
  unsupervised: "This host was not started by its supervisor, so it cannot switch versions.",
};

const REFUSED: Record<string, string> = {
  release_changed: "The published release changed after this host first saw it, so it was refused.",
  needs_manual_install: "This host is too old to update in place. Install the new version manually.",
  verification: "The release could not be verified and was not used.",
  unsupported_platform: "No build of this release exists for this host's platform.",
};

function blockersText(b: HostUpdateApply["blockers"]): string {
  const parts = [
    b.live_turns && `${b.live_turns} running turn${b.live_turns === 1 ? "" : "s"}`,
    b.pending_requests && `${b.pending_requests} waiting request${b.pending_requests === 1 ? "" : "s"}`,
    b.terminals && `${b.terminals} open terminal${b.terminals === 1 ? "" : "s"}`,
  ].filter(Boolean);
  return parts.join(", ");
}

function mb(bytes: number) {
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

/**
 * Versions and updates: this app's own (through the shell's updater) and the
 * host's (the host updates itself; this only reads its state and asks it to
 * switch). The host's answers are the truth shown here.
 */
export function AboutDialog({
  bridge,
  remote,
  onClose,
}: {
  bridge: NativeBridge;
  remote: boolean;
  onClose(): void;
}) {
  return (
    <Modal label="About Zwiim" onClose={onClose}>
      <DesktopSection bridge={bridge} />
      <HostSection remote={remote} />
      {!remote && <HostLoginToggle bridge={bridge} />}
      <DiagnosticsSection bridge={bridge} />
    </Modal>
  );
}

function DesktopSection({ bridge }: { bridge: NativeBridge }) {
  const updates = bridge.updates;
  const [status, setStatus] = useState<DesktopUpdateStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!updates) return;
    let stop = false;
    const read = () => updates.status().then((s) => !stop && setStatus(s), () => {});
    void read();
    const t = setInterval(read, 1000);
    return () => {
      stop = true;
      clearInterval(t);
    };
  }, [updates]);

  const run = (f: () => Promise<DesktopUpdateStatus | void>) => {
    setError(null);
    f().then(
      (s) => s && setStatus(s),
      (e) => setError(errorMessage(e)),
    );
  };

  const a = status?.activity;
  return (
    <section className="flex flex-col gap-1.5 rounded-md border p-2.5" data-testid="about-desktop">
      <div className="flex items-center gap-2">
        <span className="font-medium">Zwiim Desktop</span>
        <span className="text-muted-foreground font-mono">{status?.version ?? DESKTOP_CLIENT.version}</span>
      </div>
      {!updates ? null : !status ? null : !status.supported ? (
        <p className="text-muted-foreground">This build was not made by the release pipeline, so it does not update itself.</p>
      ) : (
        <>
          <div className="flex items-center gap-2">
            <span className="text-muted-foreground min-w-0 flex-1">
              {a?.state === "checking" && "Checking for updates…"}
              {a?.state === "up_to_date" && "Up to date."}
              {a?.state === "available" && `Version ${a.version} is available.`}
              {a?.state === "downloading" &&
                `Downloading ${a.version}… ${mb(a.downloaded)}${a.total ? ` of ${mb(a.total)}` : ""}`}
              {a?.state === "ready" && `Version ${a.version} is installed. Restart the app to use it; the host keeps running.`}
              {a?.state === "failed" && <span className="text-destructive">{a.message}</span>}
            </span>
            {(a?.state === "idle" || a?.state === "up_to_date" || a?.state === "failed") && (
              <Button size="xs" variant="outline" onClick={() => run(() => updates.check())}>
                Check for updates
              </Button>
            )}
            {a?.state === "available" && (
              <Button size="xs" onClick={() => run(() => updates.install())}>
                Download and install
              </Button>
            )}
            {a?.state === "ready" && (
              <Button size="xs" onClick={() => run(() => updates.restart())}>
                Restart Zwiim
              </Button>
            )}
          </div>
          {a?.state === "available" && a.notes && (
            <p className="text-muted-foreground line-clamp-4 whitespace-pre-wrap">{a.notes}</p>
          )}
        </>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

function HostSection({ remote }: { remote: boolean }) {
  const controller = useController();
  const hello = useHost((s) => s.host);
  const compat = useHost((s) => s.connection.compat);
  const [status, setStatus] = useState<HostUpdateStatus | null>(null);
  const [applied, setApplied] = useState<HostUpdateApply | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const read = useCallback(
    () => controller.client.request("host.update.status", {}).then(setStatus, () => {}),
    [controller],
  );
  useEffect(() => {
    void read();
    const t = setInterval(() => void read(), POLL_MS);
    return () => clearInterval(t);
  }, [read]);

  async function run(label: string, f: () => Promise<void>) {
    setBusy(label);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const check = () =>
    run("check", async () => {
      setStatus(await controller.client.request("host.update.check", {}));
    });
  const apply = (mode: "when_idle" | "now") =>
    run(`apply:${mode}`, async () => {
      setApplied(await controller.client.request("host.update.apply", { mode }));
    });

  const s = status;
  const last = s?.last_result;
  const failed = s?.activity.state === "failed" ? s.activity : null;
  return (
    <section className="flex flex-col gap-1.5 rounded-md border p-2.5" data-testid="about-host">
      <div className="flex items-center gap-2">
        <span className="font-medium">{remote ? "Host" : "Host on this computer"}</span>
        <span className="text-muted-foreground font-mono">{hello?.version ?? "—"}</span>
      </div>
      <dl className="text-muted-foreground grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
        <dt>Protocol</dt>
        <dd className="font-mono">
          {hello ? `${hello.protocol} · revision ${hello.revision ?? 0}` : "—"}
        </dd>
        <dt>Compatibility</dt>
        <dd data-testid="about-compat">{compat ? COMPAT[compat.status] : "—"}</dd>
        {s && (
          <>
            <dt>Channel</dt>
            <dd className="font-mono">{s.channel}</dd>
          </>
        )}
      </dl>
      {compat?.status === "update_required" && (
        <p className="text-muted-foreground">{updateRequiredMessage(compat, remote)}</p>
      )}
      {compat?.status === "limited" && (
        <p className="text-muted-foreground">
          Not available on this host: <span className="font-mono">{compat.missing.join(", ")}</span>
        </p>
      )}
      {s && !s.supported && (
        <p className="text-muted-foreground" data-testid="host-update-unsupported">
          {UNSUPPORTED[s.unsupported_reason ?? ""] ?? "This host does not update itself."}
        </p>
      )}
      {s?.trial && (
        <p className="text-muted-foreground" data-testid="host-update-trial">
          Checking version {s.trial.version} after the update. If it fails, the host returns to {s.trial.from}.
        </p>
      )}
      {last?.outcome === "rolled_back" && !s?.trial && (
        <p className="text-destructive" data-testid="host-update-rolled-back">
          The update to {last.version} failed, so the host went back to {last.from}.
          {last.reason ? ` ${last.reason}` : ""}
        </p>
      )}
      {s?.supported && (
        <div className="flex items-center gap-2">
          <span className="text-muted-foreground min-w-0 flex-1" data-testid="host-update-state">
            {s.staged
              ? `Version ${s.staged.version} is ready. Restarting the host finishes the update; sessions and workspaces are kept.`
              : s.activity.state === "checking"
                ? "Checking for updates…"
                : s.activity.state === "downloading"
                  ? `Downloading ${s.activity.version}…`
                  : s.available
                    ? s.bundled_with_desktop
                      ? `Version ${s.available.version} is available. It comes with the next Desktop update.`
                      : `Version ${s.available.version} is available.`
                    : s.last_checked
                      ? "Up to date."
                      : null}
          </span>
          {s.staged ? (
            <Button size="xs" disabled={busy !== null} onClick={() => void apply("when_idle")}>
              {busy === "apply:when_idle" ? "Restarting…" : "Restart host"}
            </Button>
          ) : (
            <Button size="xs" variant="outline" disabled={busy !== null} onClick={() => void check()}>
              {busy === "check" ? "Checking…" : "Check for updates"}
            </Button>
          )}
        </div>
      )}
      {failed && (
        <p className="text-muted-foreground break-words">
          {REFUSED[failed.reason] ?? "The last update check failed."} <span className="font-mono">{failed.message}</span>
        </p>
      )}
      {applied?.state === "restart_pending" && (
        <div className="flex items-center gap-2" data-testid="host-update-pending">
          <span className="text-muted-foreground min-w-0 flex-1">
            The host restarts when {blockersText(applied.blockers) || "its work"} finishes.
          </span>
          <Button size="xs" variant="destructive" disabled={busy !== null} onClick={() => void apply("now")}>
            Restart now
          </Button>
        </div>
      )}
      {applied?.state === "restart_pending" && (
        <p className="text-muted-foreground">Restarting now stops running turns and closes terminals.</p>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** Whether the host on this computer starts at login. Hidden where it cannot. */
function HostLoginToggle({ bridge }: { bridge: NativeBridge }) {
  const login = bridge.hostLogin;
  const [status, setStatus] = useState<HostLoginStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    login?.status().then(setStatus, () => {});
  }, [login]);
  if (!login || !status?.supported) return null;
  return (
    <section className="flex flex-col gap-1 rounded-md border p-2.5">
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          data-testid="host-login"
          checked={status.enabled}
          onChange={(e) => {
            setError(null);
            login.set(e.target.checked).then(setStatus, (err) => setError(errorMessage(err)));
          }}
        />
        <span className="font-medium">Start the host when you log in</span>
      </label>
      <p className="text-muted-foreground">
        So your other devices can reach this computer without opening the app. Takes effect at your next login.
      </p>
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

const SCHEMA: Record<HostDiagnostics["database"]["schema"]["state"], string> = {
  fresh: "new",
  current: "up to date",
  pending: "migrations pending",
  newer: "from a newer version",
};

function adapterText(a: HostDiagnostics["adapters"][string]): string {
  if (!a.installed) return a.setup.state === "installing" ? "setting up" : "not installed";
  const health = a.healthy === null ? "not checked yet" : a.healthy ? "healthy" : "unhealthy";
  return `${a.version ?? "?"} · ${health}${a.authenticated ? "" : " · not signed in"}`;
}

/**
 * What support needs, on request: the host's own report (versions, protocol,
 * adapters, update and migration state; it never includes secrets) and this
 * app's version. Copied as JSON.
 */
function DiagnosticsSection({ bridge }: { bridge: NativeBridge }) {
  const controller = useController();
  const [d, setD] = useState<HostDiagnostics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const load = () => {
    setError(null);
    controller.client.request("host.diagnostics", {}).then(setD, (e) => setError(errorMessage(e)));
  };
  const copy = async () => {
    const desktop = await bridge.updates?.status().catch(() => null);
    const report = {
      desktop: { version: desktop?.version ?? DESKTOP_CLIENT.version, updates: desktop?.activity ?? null },
      host: d,
    };
    await navigator.clipboard?.writeText(JSON.stringify(report, null, 2));
    setCopied(true);
  };

  return (
    <section className="flex flex-col gap-1.5 rounded-md border p-2.5" data-testid="about-diagnostics">
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 font-medium">Diagnostics</span>
        {d ? (
          <Button size="xs" variant="outline" onClick={() => void copy()}>
            {copied ? "Copied" : "Copy"}
          </Button>
        ) : (
          <Button size="xs" variant="outline" onClick={load}>
            Show
          </Button>
        )}
      </div>
      {d && (
        <dl className="text-muted-foreground grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
          <dt>Host</dt>
          <dd className="font-mono">
            {d.version} · {d.target ?? "unknown platform"} · {d.channel}
          </dd>
          <dt>Protocol</dt>
          <dd className="font-mono">
            revision {d.protocol.revision} · clients from {d.protocol.min_client_revision}
          </dd>
          {Object.entries(d.adapters).map(([name, a]) => (
            <div key={name} className="contents">
              <dt className="capitalize">{name}</dt>
              <dd className="font-mono" data-testid={`diagnostics-adapter-${name}`}>
                {adapterText(a)}
              </dd>
            </div>
          ))}
          <dt>Updates</dt>
          <dd>
            {d.update.trial
              ? `on trial: ${d.update.trial.version}`
              : d.update.last_result
                ? `${d.update.last_result.outcome === "rolled_back" ? "rolled back from" : "updated to"} ${d.update.last_result.version}`
                : d.update.supported
                  ? "no updates applied yet"
                  : "not updated by itself"}
          </dd>
          <dt>Database</dt>
          <dd data-testid="diagnostics-database">
            {SCHEMA[d.database.schema.state]} · {d.database.migrations_known} migrations
            {d.database.applied_at_start ? ` · ${d.database.applied_at_start} applied at start` : ""}
            {` · ${d.database.snapshots} snapshot${d.database.snapshots === 1 ? "" : "s"}`}
          </dd>
        </dl>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/**
 * Whether anything about versions wants attention: an update ready or
 * available for this app, or one staged on the host. Checks this app's
 * update once shortly after launch.
 */
export function useUpdateHint(bridge: NativeBridge): boolean {
  const controller = useController();
  const live = useHost((s) => s.connection.status === "live");
  const [desktop, setDesktop] = useState(false);
  const [host, setHost] = useState(false);
  useEffect(() => {
    const updates = bridge.updates;
    if (!updates) return;
    const t = setTimeout(() => {
      updates.check().then(
        (s) => setDesktop(s.activity.state === "available" || s.activity.state === "ready"),
        () => {},
      );
    }, 10_000);
    return () => clearTimeout(t);
  }, [bridge]);
  useEffect(() => {
    if (!live) return;
    const read = () =>
      controller.client.request("host.update.status", {}).then(
        (s) => setHost(s.restart_required),
        () => {},
      );
    void read();
    const t = setInterval(() => void read(), 10 * 60_000);
    return () => clearInterval(t);
  }, [controller, live]);
  return desktop || host;
}
