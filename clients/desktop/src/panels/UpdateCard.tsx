import { useCallback, useEffect, useState } from "react";
import { X } from "lucide-react";
import type { HostController, HostRestartMode, HostUpdateApply, HostUpdateStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { DESKTOP_CLIENT } from "@/lib/app-info";
import type { DesktopUpdates, DesktopUpdateStatus, NativeBridge } from "@/lib/bridge";
import { errorMessage, updateRequiredMessage, useController, useHost } from "@/lib/host-context";
import { blockersText, mb } from "@/panels/About";
import { ProgressBar } from "@/panels/GithubCloneDialog";
import {
  compareVersions,
  flowTiming,
  hostPreparedFor,
  isUnified,
  resetRestartFlow,
  restartNow,
  restartWhenIdle,
  useRestartPhase,
  useRestartWatch,
  waitAgain,
  agentsWorking,
  type Blockers,
  type RestartTarget,
} from "@/panels/update-flow";

/** How often the host's update state is read while connected. */
const HOST_POLL_MS = 10 * 60_000;
/** How often this app looks for a newer version while it stays open. */
export const DESKTOP_CHECK_MS = 6 * 60 * 60_000;

export interface Updates {
  desktop: DesktopUpdateStatus | null;
  host: HostUpdateStatus | null;
  setDesktop(status: DesktopUpdateStatus): void;
  /** This app and the host on this computer update as one. */
  unified: boolean;
  /** Something about versions wants attention. */
  hint: boolean;
  /** Manual checks made; each one shows a dismissed update again. */
  checks: number;
  /**
   * Check this app (and the host, unless they update as one) now (and whether
   * an update still blocks the host). True when an update wants the user.
   */
  check(): Promise<boolean>;
}

/**
 * This app's update (checked shortly after launch and every few hours after,
 * followed once one is available, so a download started from About shows here
 * too) and the host's (read while connected, again on each reconnect).
 */
export function useUpdates(bridge: NativeBridge, remote: boolean): Updates {
  const controller = useController();
  const connection = useHost((s) => s.connection.status);
  // A host that needs an update is still asked what it has staged.
  const connected = connection === "live" || connection === "incompatible";
  const [desktop, setDesktop] = useState<DesktopUpdateStatus | null>(null);
  const [host, setHost] = useState<HostUpdateStatus | null>(null);
  const [checks, setChecks] = useState(0);
  const updates = bridge.updates;

  useEffect(() => {
    if (!updates) return;
    const check = () => void updates.check().then(setDesktop, () => {});
    const first = setTimeout(check, 10_000);
    const every = setInterval(check, DESKTOP_CHECK_MS);
    return () => {
      clearTimeout(first);
      clearInterval(every);
    };
  }, [updates]);

  const state = desktop?.activity.state;
  const following = state === "available" || state === "downloading";
  useEffect(() => {
    if (!updates || !following) return;
    const t = setInterval(() => updates.status().then(setDesktop, () => {}), 1000);
    return () => clearInterval(t);
  }, [updates, following]);

  // Installing this app's update stages the host that comes with it: read again.
  const ready = state === "ready";
  useEffect(() => {
    if (!connected) return;
    const read = () => controller.client.request("host.update.status", {}).then(setHost, () => {});
    void read();
    const t = setInterval(() => void read(), HOST_POLL_MS);
    return () => clearInterval(t);
  }, [controller, connected, ready]);

  useRestartWatch(controller);
  // A restart that leaves this app open ends when the host is back: on the new
  // version, or, once it has been seen down, on any other (its trial rolled back).
  const phase = useRestartPhase();
  const finishing = phase.kind === "ready" || (phase.kind === "restarting" && !phase.target.desktop) ? phase.target : null;
  const asking = phase.kind === "ready";
  useEffect(() => {
    if (!finishing?.host) return;
    let alive = true;
    let down = false;
    const t = setInterval(
      () =>
        void controller.client.request("host.update.status", {}).then(
          (s) => {
            if (!alive) return;
            setHost(s);
            // Down then up is the restart (or its rollback) only once it was asked for.
            if (s.version === finishing.version || (down && !asking)) resetRestartFlow();
            // An agent started while the user was asked: that is waiting again.
            else if (asking && s.blockers && agentsWorking(s.blockers)) waitAgain(finishing, s.blockers);
          },
          () => {
            down = true;
          },
        ),
      flowTiming.watchMs,
    );
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [controller, finishing, asking]);

  const unified = isUnified(host, remote);
  const check = useCallback(async () => {
    const blocked = controller.store.getState().connection.status === "incompatible";
    // Connecting again is how a blocked host's compatibility is looked at anew.
    if (blocked) controller.client.retryNow();
    // This app's update brings the host's: the host only says what it has.
    const [d, h] = await Promise.all([
      updates ? updates.check().catch(() => null) : null,
      blocked && !unified
        ? null
        : controller.client.request(unified ? "host.update.status" : "host.update.check", {}).catch(() => null),
    ]);
    if (d) setDesktop(d);
    if (h) setHost(h);
    setChecks((n) => n + 1);
    const a = d?.activity.state;
    return blocked || Boolean(h?.restart_required) || a === "available" || a === "ready";
  }, [controller, updates, unified]);

  const hint = Boolean(host?.restart_required) || state === "available" || state === "ready";
  return { desktop, host, setDesktop, unified, hint, checks, check };
}

interface Card {
  /** What was dismissed; a newer state shows again. Unset: cannot be dismissed. */
  key?: string;
  title: string;
  detail?: string;
  actions?: React.ReactNode;
  /** This app and the host restart together: it replaces the detail and the actions. */
  choices?: { target: RestartTarget; blockers: Blockers | null; detail?: string };
  /** While this app's update downloads. */
  progress?: { percent: number | null; label: string };
}

type Downloading = Extract<DesktopUpdateStatus["activity"], { state: "downloading" }>;

/**
 * How far a download is: a percent once the size is known. Once it is all
 * here, the update is being verified and installed.
 */
export function downloadProgress(d: Downloading): { percent: number | null; label: string } {
  if (d.total && d.downloaded >= d.total) return { percent: 100, label: "Installing…" };
  const percent = d.total ? Math.min(100, Math.floor((d.downloaded / d.total) * 100)) : null;
  const size = d.total ? `${mb(d.downloaded)} of ${mb(d.total)} (${percent}%)` : mb(d.downloaded);
  return { percent, label: `Downloading… ${size}` };
}

const secondary = "h-7 px-2.5 text-xs";

/**
 * Restart onto an installed Desktop update when this app and its host do not
 * update as one: the host on this computer switches to the version staged
 * with it once its running work finishes, and this app relaunches now. A host
 * that cannot switch, or a remote one, is left as it is.
 */
export async function restartIntoUpdate(controller: HostController, updates: DesktopUpdates, remote: boolean) {
  if (!remote) {
    const host = await controller.client.request("host.update.status", {}).catch(() => null);
    if (host?.restart_required && !host.restart_pending) {
      await controller.client.request("host.update.apply", { mode: "when_idle" }).catch(() => {});
    }
  }
  await updates.restart();
}

/**
 * What restarting finishes for a ready Desktop update: this app, and the host
 * too when it runs that version or has it staged. Otherwise this app alone.
 */
export function readyTarget(host: HostUpdateStatus, desktop: DesktopUpdates, version: string): RestartTarget {
  return { host: hostPreparedFor(host, version) && host.version !== version, desktop, version };
}

/** What Restart now stops, said by the agents known to be working. */
function stopsText(b: Blockers | null): string {
  // A turn and a request on one session are one agent; an older Host names no sessions.
  const n = !b ? 0 : b.session_ids.length ? b.session_ids.length + (b.starting ?? 0) : b.live_turns + b.pending_requests + (b.starting ?? 0);
  return n ? `Stops ${n} running agent${n === 1 ? "" : "s"} and closes open terminals.` : "Stops running agents and closes open terminals.";
}

/**
 * How to restart onto an update that is ready: now, or once no agent works
 * (then the user is asked, never relaunched). Follows the shared restart
 * phase, so every view of it agrees. `detail` says what the update is while
 * nothing waits.
 */
export function UpdateChoices({
  target,
  blockers,
  detail,
}: {
  target: RestartTarget;
  blockers: Blockers | null;
  detail?: string;
}) {
  const controller = useController();
  const phase = useRestartPhase();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const off = busy || phase.kind === "restarting";

  async function go(f: () => Promise<void>) {
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
  const now = (
    <Button
      className={secondary}
      variant="outline"
      disabled={off}
      title={stopsText(phase.kind === "waiting" ? phase.blockers : blockers)}
      onClick={() => void go(() => restartNow(controller, target))}
    >
      Restart now
    </Button>
  );

  return (
    <div className="flex flex-col gap-2.5">
      <p className="text-muted-foreground text-xs" data-testid="update-detail">
        {phase.kind === "waiting"
          ? `Waiting for ${(phase.blockers && blockersText(phase.blockers)) || "agents"} to finish.`
          : phase.kind === "ready"
            ? `Agents are done. Restart to finish updating to ${target.version}.`
            : (detail ??
              `Zwiim ${target.version} is downloaded for this app and its host. Sessions and workspaces are kept.`)}
      </p>
      <div className="flex flex-wrap gap-1.5">
        {phase.kind === "ready" ? (
          <Button className={secondary} variant="brand" disabled={off} onClick={() => void go(() => restartWhenIdle(controller, target))}>
            Restart
          </Button>
        ) : phase.kind === "waiting" ? (
          now
        ) : (
          <>
            <Button className={secondary} variant="brand" disabled={off} onClick={() => void go(() => restartWhenIdle(controller, target))}>
              Restart when idle
            </Button>
            {now}
          </>
        )}
      </div>
      {error && (
        <p className="text-destructive text-xs break-words" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * An update that wants the user, above the account menu: this app's update
 * (which brings the host's), the host's own, or the host on this computer
 * being older than this app. Where the host is this computer's own, Update
 * downloads and installs both, and the restart that finishes them is offered
 * with its choices. Otherwise one Update does all of it: this app downloads,
 * installs and relaunches, and the host restarts once its running work
 * finishes. Restarting the host now, which stops running turns, is offered
 * only while it waits.
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
  // Update was clicked while this app's update downloads: finish it once installed.
  // Only when this app and its host do not update as one; there, restarting is a choice.
  const [finishing, setFinishing] = useState(false);
  const phase = useRestartPhase();
  const unified = updates.unified;

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

  const h = updates.host;
  // Asked of this host, here or by an earlier window of this app.
  const waiting = applied ? applied.state === "restart_pending" : Boolean(h?.restart_pending);
  const hostActions = waiting ? (
    <Button
      className={secondary}
      variant="brand"
      disabled={busy}
      title="Stops running turns and closes terminals"
      onClick={() => void restartHost("now")}
    >
      Restart now
    </Button>
  ) : (
    !applied && (
      <Button className={secondary} variant="brand" disabled={busy} onClick={() => void restartHost("when_idle")}>
        Update
      </Button>
    )
  );
  const hostDetail = (fallback: string) =>
    applied?.state === "restart_pending"
      ? `The host restarts when ${blockersText(applied.blockers) || "its work"} finishes.`
      : applied?.state === "restarting"
        ? "Restarting the host…"
        : waiting
          ? "The host restarts when its running work finishes."
          : fallback;

  const seeChanges = (
    <Button className={secondary} variant="outline" onClick={onSeeChanges}>
      See changes
    </Button>
  );

  const d = updates.desktop?.activity;
  const desktopUpdates = bridge.updates;

  const state = d?.state;
  useEffect(() => {
    if (!finishing || state === "available" || state === "downloading") return;
    setFinishing(false);
    if (state === "ready" && desktopUpdates) void run(() => restartIntoUpdate(controller, desktopUpdates, remote));
  }, [finishing, state, desktopUpdates, controller, remote]);

  const desktopVersion = updates.desktop?.version ?? DESKTOP_CLIENT.version;
  const choicesBlockers = h?.blockers ?? null;
  let card: Card | null = null;
  const desktopActions =
    desktopUpdates && (d?.state === "ready" || d?.state === "available") ? (
      <Button
        className={secondary}
        variant="brand"
        disabled={busy}
        onClick={() =>
          void run(async () => {
            if (d.state === "ready") return restartIntoUpdate(controller, desktopUpdates, remote);
            setFinishing(!unified);
            try {
              updates.setDesktop(await desktopUpdates.install());
            } catch (e) {
              setFinishing(false);
              throw e;
            }
          })
        }
      >
        Update
      </Button>
    ) : null;
  const desktopDetail = (version: string) =>
    remote
      ? `Zwiim Desktop ${version}.`
      : `Zwiim ${version} for this app and its host. Sessions and workspaces are kept.`;
  const required = conn.status === "incompatible" && conn.compat?.status === "update_required" ? conn.compat : null;
  const staged = h?.supported ? h.staged : null;
  if (unified && h && phase.kind !== "idle" && phase.target.host) {
    // Restarting is under way, or waits for agents or for the user.
    const hostOnly = !phase.target.desktop;
    card = {
      title: phase.kind === "ready" ? "Ready to restart" : hostOnly ? "Finish update" : "Update ready",
      choices: {
        target: phase.target,
        blockers: choicesBlockers,
        detail: hostOnly ? `This app is on ${desktopVersion}. Its host is still on ${h.version}.` : undefined,
      },
    };
  } else if (unified && h && desktopUpdates && d?.state === "ready") {
    card = hostPreparedFor(h, d.version)
      ? {
          key: `desktop:${d.version}`,
          title: "Update ready",
          choices: { target: readyTarget(h, desktopUpdates, d.version), blockers: choicesBlockers },
        }
      : {
          key: `desktop:${d.version}`,
          title: "Update ready",
          detail: `Zwiim ${d.version} is downloaded for this app, but its host couldn't be prepared (it stays on ${h.version}). Restart to update this app.`,
          actions: (
            <Button
              className={secondary}
              variant="brand"
              disabled={busy || phase.kind === "restarting"}
              onClick={() => void run(() => restartNow(controller, readyTarget(h, desktopUpdates, d.version)))}
            >
              Restart Zwiim
            </Button>
          ),
        };
  } else if (
    unified &&
    h &&
    staged &&
    compareVersions(h.version, desktopVersion) < 0 &&
    compareVersions(staged.version, desktopVersion) >= 0
  ) {
    // This app is new and its host is not: the update was left unfinished.
    card = {
      title: "Finish update",
      choices: {
        target: { host: true, desktop: null, version: staged.version },
        blockers: choicesBlockers,
        detail: `This app is on ${desktopVersion}. Its host is still on ${h.version}.`,
      },
    };
  } else if (required?.update === "host" && !remote) {
    card = {
      title: "Host update required",
      detail: hostDetail("The host on this computer is older than this app. Restart it to run the new version."),
      actions: hostActions,
    };
  } else if (required) {
    card = {
      title: required.update === "host" ? "Host update required" : "App update required",
      detail: updateRequiredMessage(required, remote),
      actions: required.update === "client" && desktopActions,
      progress: required.update === "client" && d?.state === "downloading" ? downloadProgress(d) : undefined,
    };
  } else if (desktopUpdates && (d?.state === "available" || d?.state === "downloading" || d?.state === "ready")) {
    // This app's update brings the host's, so it covers a host update too.
    card = {
      key: `desktop:${d.version}`,
      title: "New update available",
      detail: desktopDetail(d.version),
      actions: (
        <>
          {seeChanges}
          {desktopActions}
        </>
      ),
      progress: d.state === "downloading" ? downloadProgress(d) : undefined,
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
  }

  if (!card || (card.key && card.key === dismissed)) return null;
  return (
    <div
      className="bg-background ring-border relative mx-1 mb-2 flex flex-col gap-2.5 rounded-xl p-3 shadow-sm ring-1 animate-enter"
      role="status"
      aria-label={card.title}
      data-testid="update-card"
    >
      {card.key && (
        <button
          className="bg-background ring-border hover:bg-muted absolute -top-2 -left-2 grid size-5 place-items-center rounded-full ring-1 transition-colors"
          aria-label="Dismiss"
          title="Dismiss"
          onClick={() => setDismissed(card.key!)}
        >
          <X className="size-3" />
        </button>
      )}
      {card.choices ? (
        <div className="flex flex-col gap-0.5">
          <p className="text-ui font-medium">{card.title}</p>
          <UpdateChoices {...card.choices} />
        </div>
      ) : (
        <>
          <div className="flex flex-col gap-0.5">
            <p className="text-ui font-medium">{card.title}</p>
            <p className="text-muted-foreground text-xs" data-testid="update-detail">
              {card.detail}
            </p>
          </div>
          <div className="flex flex-wrap gap-1.5">{card.actions}</div>
        </>
      )}
      {card.progress && (
        <div className="flex flex-col gap-1" data-testid="update-progress">
          <ProgressBar percent={card.progress.percent} fill="bg-brand" label={card.progress.label} />
          <p className="text-muted-foreground text-xs tabular-nums">{card.progress.label}</p>
        </div>
      )}
      {error && (
        <p className="text-destructive text-xs break-words" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
