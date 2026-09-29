import { useCallback, useEffect, useState } from "react";
import { X } from "lucide-react";
import type { HostRestartMode, HostUpdateApply, HostUpdateStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import type { DesktopUpdateStatus, NativeBridge } from "@/lib/bridge";
import { errorMessage, updateRequiredMessage, useController, useHost } from "@/lib/host-context";
import { blockersText } from "@/panels/About";

/** How often the host's update state is read while connected. */
const HOST_POLL_MS = 10 * 60_000;

export interface Updates {
  desktop: DesktopUpdateStatus | null;
  host: HostUpdateStatus | null;
  setDesktop(status: DesktopUpdateStatus): void;
  /** Something about versions wants attention. */
  hint: boolean;
  /** Manual checks made; each one shows a dismissed update again. */
  checks: number;
  /**
   * Check this app and the host now (and whether an update still blocks the
   * host). True when an update wants the user.
   */
  check(): Promise<boolean>;
}

/**
 * This app's update (checked once shortly after launch, followed while it
 * downloads) and the host's (read while connected, again on each reconnect).
 */
export function useUpdates(bridge: NativeBridge): Updates {
  const controller = useController();
  const live = useHost((s) => s.connection.status === "live");
  const [desktop, setDesktop] = useState<DesktopUpdateStatus | null>(null);
  const [host, setHost] = useState<HostUpdateStatus | null>(null);
  const [checks, setChecks] = useState(0);
  const updates = bridge.updates;

  useEffect(() => {
    if (!updates) return;
    const t = setTimeout(() => updates.check().then(setDesktop, () => {}), 10_000);
    return () => clearTimeout(t);
  }, [updates]);

  const downloading = desktop?.activity.state === "downloading";
  useEffect(() => {
    if (!updates || !downloading) return;
    const t = setInterval(() => updates.status().then(setDesktop, () => {}), 1000);
    return () => clearInterval(t);
  }, [updates, downloading]);

  useEffect(() => {
    if (!live) return;
    const read = () => controller.client.request("host.update.status", {}).then(setHost, () => {});
    void read();
    const t = setInterval(() => void read(), HOST_POLL_MS);
    return () => clearInterval(t);
  }, [controller, live]);

  const check = useCallback(async () => {
    const blocked = controller.store.getState().connection.status === "incompatible";
    // Connecting again is how a blocked host's compatibility is looked at anew.
    if (blocked) controller.client.retryNow();
    const [d, h] = await Promise.all([
      updates ? updates.check().catch(() => null) : null,
      blocked ? null : controller.client.request("host.update.check", {}).catch(() => null),
    ]);
    if (d) setDesktop(d);
    if (h) setHost(h);
    setChecks((n) => n + 1);
    const a = d?.activity.state;
    return blocked || Boolean(h?.restart_required) || a === "available" || a === "ready";
  }, [controller, updates]);

  const d = desktop?.activity.state;
  const hint = Boolean(host?.restart_required) || d === "available" || d === "ready";
  return { desktop, host, setDesktop, hint, checks, check };
}

interface Card {
  /** What was dismissed; a newer state shows again. Unset: cannot be dismissed. */
  key?: string;
  title: string;
  detail: string;
  actions: React.ReactNode;
}

const secondary = "h-7 px-2.5 text-xs";

/**
 * An update that wants the user, above the account menu: a host restart into
 * a newer version (when idle or now), this app's update, or the host on this
 * computer being older than this app. The host decides when it restarts;
 * restarting it now stops running turns.
 */
export function UpdateCard({
  bridge,
  updates,
  remote,
  onSeeChanges,
}: {
  bridge: NativeBridge;
  updates: Updates;
  remote: boolean;
  onSeeChanges(): void;
}) {
  const controller = useController();
  const conn = useHost((s) => s.connection);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [applied, setApplied] = useState<HostUpdateApply | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setDismissed(null), [updates.checks]);

  // A restart ends with a new connection: what was asked of the old host is done.
  useEffect(() => {
    if (conn.status === "live") setApplied(null);
  }, [conn.status]);

  async function run(f: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const restartHost = (mode: HostRestartMode) =>
    run(async () => {
      setApplied(await controller.client.request("host.update.apply", { mode }));
    });

  const hostActions = applied ? (
    applied.state === "restart_pending" && (
      <Button
        className={secondary}
        disabled={busy}
        title="Stops running turns and closes terminals"
        onClick={() => void restartHost("now")}
      >
        Restart now
      </Button>
    )
  ) : (
    <>
      <Button className={secondary} variant="outline" disabled={busy} onClick={() => void restartHost("when_idle")}>
        Restart when idle
      </Button>
      <Button
        className={secondary}
        disabled={busy}
        title="Stops running turns and closes terminals"
        onClick={() => void restartHost("now")}
      >
        Restart
      </Button>
    </>
  );
  const hostDetail = (fallback: string) =>
    applied?.state === "restart_pending"
      ? `The host restarts when ${blockersText(applied.blockers) || "its work"} finishes.`
      : applied?.state === "restarting"
        ? "Restarting the host…"
        : fallback;

  const seeChanges = (
    <Button className={secondary} variant="outline" onClick={onSeeChanges}>
      See changes
    </Button>
  );

  const h = updates.host;
  const d = updates.desktop?.activity;
  const desktopUpdates = bridge.updates;
  let card: Card | null = null;
  const desktopActions =
    desktopUpdates && d?.state === "ready" ? (
      <Button className={secondary} disabled={busy} onClick={() => void run(() => desktopUpdates.restart())}>
        Restart
      </Button>
    ) : desktopUpdates && d?.state === "available" ? (
      <Button
        className={secondary}
        disabled={busy}
        onClick={() => void run(async () => updates.setDesktop(await desktopUpdates.install()))}
      >
        Update
      </Button>
    ) : null;
  const required = conn.status === "incompatible" && conn.compat?.status === "update_required" ? conn.compat : null;
  if (required?.update === "host" && !remote) {
    card = {
      title: "Host update required",
      detail: hostDetail("The host on this computer is older than this app. Restart it to run the new version."),
      actions: hostActions,
    };
  } else if (required) {
    card = {
      title: required.update === "host" ? "Host update required" : "App update required",
      detail:
        required.update === "client" && d?.state === "downloading"
          ? `Downloading Zwiim Desktop ${d.version}…`
          : updateRequiredMessage(required, remote),
      actions: required.update === "client" && desktopActions,
    };
  } else if (h?.restart_required && h.staged) {
    card = {
      key: `host:${h.staged.version}`,
      title: "New update available",
      detail: hostDetail(`Zwiim Host ${h.staged.version}. Sessions and workspaces are kept.`),
      actions: (
        <>
          {seeChanges}
          {hostActions}
        </>
      ),
    };
  } else if (desktopUpdates && d?.state === "ready") {
    card = {
      key: `desktop-ready:${d.version}`,
      title: "New update available",
      detail: `Zwiim Desktop ${d.version} is installed. The host keeps running.`,
      actions: (
        <>
          {seeChanges}
          {desktopActions}
        </>
      ),
    };
  } else if (desktopUpdates && (d?.state === "available" || d?.state === "downloading")) {
    card = {
      key: `desktop:${d.version}`,
      title: "New update available",
      detail: d.state === "downloading" ? `Downloading Zwiim Desktop ${d.version}…` : `Zwiim Desktop ${d.version}.`,
      actions: (
        <>
          {seeChanges}
          {desktopActions}
        </>
      ),
    };
  }

  if (!card || (card.key && card.key === dismissed)) return null;
  return (
    <div
      className="bg-background ring-border relative mx-1 mb-2 flex flex-col gap-2.5 rounded-xl p-3 shadow-sm ring-1"
      role="status"
      aria-label={card.title}
      data-testid="update-card"
    >
      {card.key && (
        <button
          className="bg-background ring-border hover:bg-muted absolute -top-2 -left-2 grid size-5 place-items-center rounded-full ring-1"
          aria-label="Dismiss"
          title="Dismiss"
          onClick={() => setDismissed(card.key!)}
        >
          <X className="size-3" />
        </button>
      )}
      <div className="flex flex-col gap-0.5">
        <p className="text-[13px] font-medium">{card.title}</p>
        <p className="text-muted-foreground text-xs" data-testid="update-detail">
          {card.detail}
        </p>
      </div>
      <div className="flex flex-wrap gap-1.5">{card.actions}</div>
      {error && (
        <p className="text-destructive text-xs break-words" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
