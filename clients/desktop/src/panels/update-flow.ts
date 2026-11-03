import { useEffect, useSyncExternalStore } from "react";
import type { HostController, HostUpdateApply, HostUpdateStatus } from "@zwiim/sdk";
import type { DesktopUpdates } from "@/lib/bridge";

export type Blockers = HostUpdateApply["blockers"];

/** What restarting finishes: the host on this computer, this app, or both. */
export type RestartTarget = { host: boolean; desktop: DesktopUpdates | null; version: string };

export type RestartPhase =
  | { kind: "idle" }
  | { kind: "restarting"; target: RestartTarget }
  | { kind: "waiting"; target: RestartTarget; blockers: Blockers | null }
  | { kind: "ready"; target: RestartTarget };

export const flowTiming = { watchMs: 3000 };

/** Semver order, where a prerelease comes before its release and numeric parts compare as numbers. */
export function compareVersions(a: string, b: string): number {
  const parse = (v: string) => {
    const [core, pre] = v.split("+")[0]!.split(/-(.*)/s);
    return { nums: core!.split(".").map(Number), pre: pre ? pre.split(".") : [] };
  };
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < Math.max(x.nums.length, y.nums.length); i++) {
    const d = (x.nums[i] ?? 0) - (y.nums[i] ?? 0);
    if (d) return Math.sign(d);
  }
  if (!x.pre.length || !y.pre.length) return Math.sign(y.pre.length - x.pre.length);
  const num = (s: string) => /^\d+$/.test(s);
  for (let i = 0; i < Math.min(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i]!;
    const q = y.pre[i]!;
    if (p === q) continue;
    if (num(p) && num(q)) return Math.sign(Number(p) - Number(q));
    if (num(p) !== num(q)) return num(p) ? -1 : 1;
    return p < q ? -1 : 1;
  }
  return Math.sign(x.pre.length - y.pre.length);
}

/** Desktop updates the Host on this computer too, as one. */
export function isUnified(host: HostUpdateStatus | null, remote: boolean): boolean {
  return !remote && host?.bundled_with_desktop === true;
}

/** The Host runs `version`, or has it staged and can switch to it. */
export function hostPreparedFor(host: HostUpdateStatus, version: string): boolean {
  return host.version === version || (host.supported && host.staged?.version === version);
}

/** Open terminals do not count: a restart closes them. */
export function agentsWorking(b: Blockers): boolean {
  return b.live_turns + b.pending_requests + (b.starting ?? 0) > 0;
}

let phase: RestartPhase = { kind: "idle" };
const listeners = new Set<() => void>();

function setPhase(next: RestartPhase) {
  phase = next;
  listeners.forEach((l) => l());
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

/** Where the restart is, shared by every reader in this window. */
export function useRestartPhase(): RestartPhase {
  return useSyncExternalStore(subscribe, () => phase);
}

export function resetRestartFlow() {
  setPhase({ kind: "idle" });
}

/** Stops running turns and closes terminals; so call it when none work, or when the user said now. */
async function run(controller: HostController, target: RestartTarget, applyHost: boolean) {
  const back = phase;
  setPhase({ kind: "restarting", target });
  try {
    if (applyHost) await controller.client.request("host.update.apply", { mode: "now" });
    await target.desktop?.restart();
  } catch (e) {
    setPhase(back);
    throw e;
  }
}

export const restartNow = (controller: HostController, target: RestartTarget) => run(controller, target, target.host);

/** Restart now unless an agent is working; otherwise wait for it, and for the user. */
export async function restartWhenIdle(controller: HostController, target: RestartTarget) {
  if (!target.host) return restartNow(controller, target);
  const { blockers, restarting } = await readBlockers(controller);
  // An older Host that was free to restart is already doing it.
  if (restarting) return run(controller, target, false);
  if (!blockers || !agentsWorking(blockers)) return restartNow(controller, target);
  setPhase({ kind: "waiting", target, blockers });
}

/** An agent started while the user was asked: wait for it again. */
export function waitAgain(target: RestartTarget, blockers: Blockers) {
  setPhase({ kind: "waiting", target, blockers });
}

interface Read {
  version: string;
  blockers: Blockers | null;
  restarting: boolean;
}

async function read(controller: HostController): Promise<Read> {
  const status = await controller.client.request("host.update.status", {});
  if (status.blockers) return { version: status.version, blockers: status.blockers, restarting: false };
  // An older Host reports blockers only with the restart it arms.
  const applied = await controller.client.request("host.update.apply", { mode: "when_idle" });
  const restarting = applied.state === "restarting";
  return { version: status.version, blockers: restarting ? null : applied.blockers, restarting };
}

export async function readBlockers(controller: HostController): Promise<{ blockers: Blockers | null; restarting: boolean }> {
  const { blockers, restarting } = await read(controller);
  return { blockers, restarting };
}

/**
 * While waiting, read the Host until no agent has worked for two reads in a
 * row, then ask the user. Mount once.
 */
export function useRestartWatch(controller: HostController) {
  const current = useRestartPhase();
  const target = current.kind === "waiting" ? current.target : null;
  useEffect(() => {
    if (!target) return;
    let live = true;
    let reading = false;
    let quiet = 0;
    const tick = async () => {
      if (reading) return;
      reading = true;
      try {
        const r = await read(controller);
        if (!live) return;
        // An older Host restarted by itself: finish with this app.
        if (r.restarting || (target.host && r.version === target.version)) {
          await run(controller, target, false);
        } else if (r.blockers && agentsWorking(r.blockers)) {
          quiet = 0;
          setPhase({ kind: "waiting", target, blockers: r.blockers });
        } else if (++quiet >= 2) {
          setPhase({ kind: "ready", target });
        }
      } catch {
        // The Host may be down for its restart; read again.
        quiet = 0;
      } finally {
        reading = false;
      }
    };
    const t = setInterval(() => void tick(), flowTiming.watchMs);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [controller, target]);
}
