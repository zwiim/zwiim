import { useEffect, useId, useState } from "react";
import { Check, ChevronsUpDown, Globe, Loader2, Monitor, X } from "lucide-react";
import type { CloudHost, ConnectionMode } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import type { NativeBridge, PairedHost } from "@/lib/bridge";
import { errorMessage } from "@/lib/host-context";
import type { HostChoice } from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { hostLabel, listCloudHosts, pairRemote } from "@/lib/remote";
import { cn } from "@/lib/utils";

/** The host a pairing is for; none when the user pastes any host's code. */
export interface PairTarget {
  hostId?: string;
  name?: string | null;
}

/** A centered dialog over the window. */
export function Modal({
  label,
  busy = false,
  onClose,
  children,
}: {
  label: string;
  busy?: boolean;
  onClose(): void;
  children: React.ReactNode;
}) {
  const titleId = useId();
  return (
    <Dialog
      className="max-h-full max-w-md gap-3 overflow-y-auto p-5 text-xs"
      aria-labelledby={titleId}
      dismissible={!busy}
      onClose={onClose}
    >
      <div className="flex items-center">
        <h2 id={titleId} className="text-xl font-semibold tracking-tight">
          {label}
        </h2>
        <Button className="ml-auto" size="icon-sm" variant="ghost" title="Close" disabled={busy} onClick={onClose}>
          <X />
        </Button>
      </div>
      {children}
    </Dialog>
  );
}

interface HostList {
  /** This computer's host, from discovery. */
  localHostId: string | null;
  signedIn: boolean;
  /** The account's hosts can be listed (a Cloud, the keychain, not local only). */
  cloudUrl: string | null;
  /** Paired hosts can be reached directly on the local network. */
  lan: boolean;
  paired: PairedHost[];
  /** The account's hosts; null when not loaded. */
  cloud: CloudHost[] | null;
  error: string | null;
}

async function loadHosts(bridge: NativeBridge, mode: ConnectionMode): Promise<HostList> {
  const [status, local] = await Promise.all([
    bridge.accountStatus().catch(() => null),
    bridge.discover().catch(() => null),
  ]);
  const remote = bridge.remote;
  // Local only never contacts the Cloud, not even to list hosts.
  const cloudUrl = (remote && mode !== "local" && status?.cloud_url) || null;
  const list: HostList = {
    localHostId: local?.host_id || null,
    signedIn: Boolean(status?.account),
    cloudUrl,
    lan: Boolean(remote?.lan) && mode !== "cloud",
    paired: [],
    cloud: null,
    error: null,
  };
  if (!remote || (!cloudUrl && !list.lan)) return list;
  try {
    list.paired = await remote.pairedHosts();
    if (cloudUrl && list.signedIn) list.cloud = await listCloudHosts(remote, cloudUrl);
  } catch (e) {
    list.error = errorMessage(e);
  }
  return list;
}

const rowClass = (selected: boolean) =>
  cn(
    "text-ui flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors disabled:cursor-not-allowed disabled:opacity-50",
    selected ? "bg-accent" : "enabled:hover:bg-muted",
  );

/**
 * Which host this window shows: this computer's, or one of the account's
 * hosts reached through Zwiim Cloud or directly on the local network (as
 * `mode` allows). The account's entry for this computer's own host is shown
 * as "This computer": it is always reached locally.
 */
export function HostPicker({
  bridge,
  mode,
  choice,
  onChoose,
  onPair,
}: {
  bridge: NativeBridge;
  mode: ConnectionMode;
  choice: HostChoice;
  onChoose(choice: HostChoice): void;
  onPair(target: PairTarget): void;
}) {
  const [open, setOpen] = useState(false);
  const pop = usePresence(open);
  // Esc closes the list, like every other menu.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);
  const [list, setList] = useState<HostList | null>(null);
  const [pinnedName, setPinnedName] = useState<string | null>(null);

  useEffect(() => {
    if (choice.kind !== "remote") return;
    bridge.remote?.pairedHosts().then(
      (hosts) => setPinnedName(hostLabel(hosts.find((h) => h.host_id === choice.hostId) ?? { host_id: choice.hostId })),
      () => setPinnedName(hostLabel({ host_id: choice.hostId })),
    );
  }, [bridge, choice]);

  useEffect(() => {
    if (!open) return;
    let live = true;
    setList(null);
    void loadHosts(bridge, mode).then((l) => live && setList(l));
    return () => {
      live = false;
    };
  }, [open, bridge, mode]);

  const current = choice.kind === "local" ? "This computer" : (pinnedName ?? "Remote host");
  const Icon = choice.kind === "local" ? Monitor : Globe;

  function pick(next: HostChoice) {
    setOpen(false);
    onChoose(next);
  }

  const pinned = (id: string) => list?.paired.find((p) => p.host_id === id);
  const others = (list?.cloud ?? []).filter((h) => h.host_id !== list?.localHostId);
  // Paired, but not among the signed-in account's hosts (or the list is unavailable).
  const orphans = (list?.paired ?? []).filter(
    (p) => p.host_id !== list?.localHostId && !others.some((h) => h.host_id === p.host_id),
  );

  return (
    <div className="relative" data-testid="host-picker">
      <button
        className="hover:bg-muted flex w-full items-center gap-2 rounded-md border px-2 py-1.5 text-left transition-colors"
        aria-haspopup="listbox"
        aria-expanded={open}
        title="Choose the host this window shows"
        onClick={() => setOpen((o) => !o)}
      >
        <Icon className="text-muted-foreground size-4 shrink-0" />
        <span className="min-w-0 flex-1 truncate font-medium" data-testid="current-host">
          {current}
        </span>
        <ChevronsUpDown className="text-muted-foreground size-3.5 shrink-0" />
      </button>
      {pop.mounted && (
        <>
          {open && <div className="fixed inset-0 z-40" onPointerDown={() => setOpen(false)} />}
          <div
            className="bg-popover shadow-pop presence-pop absolute top-full right-0 left-0 z-50 mt-1 flex origin-top flex-col gap-0.5 rounded-lg border p-1"
            data-state={pop.state}
            role="listbox"
            aria-label="Hosts"
          >
            <button
              className={rowClass(choice.kind === "local")}
              role="option"
              aria-selected={choice.kind === "local"}
              onClick={() => pick({ kind: "local" })}
            >
              <Monitor className="size-3.5 shrink-0" />
              <span className="flex-1">This computer</span>
              {choice.kind === "local" && <Check className="size-3.5" />}
            </button>
            {!list ? (
              <span className="text-muted-foreground flex items-center gap-1.5 px-2 py-1.5 text-xs">
                <Loader2 className="size-3.5 animate-spin" /> Looking for your hosts…
              </span>
            ) : !list.cloudUrl && !list.lan ? null : (
              <>
                {others.map((h) => {
                  const paired = pinned(h.host_id);
                  const selected = choice.kind === "remote" && choice.hostId === h.host_id;
                  return (
                    <button
                      key={h.host_id}
                      className={rowClass(selected)}
                      role="option"
                      aria-selected={selected}
                      title={paired ? `Switch to ${hostLabel(h)}` : `Pair with ${hostLabel(h)}`}
                      onClick={() => {
                        if (paired) pick({ kind: "remote", hostId: h.host_id });
                        else {
                          setOpen(false);
                          onPair({ hostId: h.host_id, name: h.name });
                        }
                      }}
                    >
                      <span
                        className={cn("size-2 shrink-0 rounded-full", h.online ? "bg-done" : "bg-faint")}
                        aria-label={h.online ? "online" : "offline"}
                      />
                      <span className="min-w-0 flex-1 truncate">{hostLabel(h)}</span>
                      <span className="text-muted-foreground text-xs">
                        {!paired ? "Pair…" : h.online ? "" : "Offline"}
                      </span>
                      {selected && <Check className="size-3.5" />}
                    </button>
                  );
                })}
                {orphans.map((p) => {
                  const selected = choice.kind === "remote" && choice.hostId === p.host_id;
                  // Through the Cloud, only a host the signed-in account lists
                  // can be reached; directly, any host that trusts this device.
                  const reachable = list.lan || (list.signedIn && list.cloud === null);
                  return (
                    <button
                      key={p.host_id}
                      className={rowClass(selected)}
                      role="option"
                      aria-selected={selected}
                      disabled={!reachable && !selected}
                      title={reachable ? `Switch to ${hostLabel(p)}` : "Not one of the signed-in account's hosts"}
                      onClick={() => pick({ kind: "remote", hostId: p.host_id })}
                    >
                      <Globe className="size-3.5 shrink-0" />
                      <span className="min-w-0 flex-1 truncate">{hostLabel(p)}</span>
                      <span className="text-muted-foreground text-xs">{reachable ? "" : "Not in this account"}</span>
                      {selected && <Check className="size-3.5" />}
                    </button>
                  );
                })}
                {list.cloudUrl && !list.signedIn && (
                  <span className="text-muted-foreground px-2 py-1.5 text-xs">Sign in to reach your other hosts.</span>
                )}
                {list.error && (
                  <span className="text-destructive px-2 py-1.5 text-xs" role="alert">
                    {list.error}
                  </span>
                )}
                {list.cloudUrl && list.signedIn && (
                  <button
                    className={rowClass(false)}
                    onClick={() => {
                      setOpen(false);
                      onPair({});
                    }}
                  >
                    <span className="text-muted-foreground">Pair with a host…</span>
                  </button>
                )}
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/**
 * Pair this computer with a host from the text of the host's pairing QR
 * (Devices → Pair a device, on the host). The code is sent to the host
 * inside the encrypted tunnel; it works once and expires in minutes.
 */
export function PairDialog({
  bridge,
  target,
  onClose,
  onPaired,
}: {
  bridge: NativeBridge;
  target: PairTarget;
  onClose(): void;
  onPaired(host: PairedHost): void;
}) {
  const [text, setText] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function pair() {
    const remote = bridge.remote;
    setPending(true);
    setError(null);
    try {
      const status = await bridge.accountStatus();
      if (!remote || !status.cloud_url) throw new Error("Remote hosts are not available in this build");
      if (!status.account) throw new Error("Sign in first: pairing reaches the host through your account");
      const host = await pairRemote(remote, status.cloud_url, text, {
        expectedHostId: target.hostId,
        name: target.name ?? null,
      });
      onPaired(host);
    } catch (e) {
      setError(errorMessage(e));
      setPending(false);
    }
  }

  return (
    <Modal label={target.hostId ? `Pair with ${hostLabel({ host_id: target.hostId, name: target.name })}` : "Pair with a host"} busy={pending} onClose={onClose}>
      <p className="text-muted-foreground">
        On the host's computer, open Zwiim → Devices → Pair a device, and paste the pairing code shown under its QR
        here. The code works once, for a few minutes.
      </p>
      <textarea
        className="bg-input/30 min-h-24 w-full resize-none rounded-md border p-2 font-mono text-2xs"
        aria-label="Pairing code"
        placeholder='{"v":2,"host_id":…}'
        autoFocus
        value={text}
        disabled={pending}
        onChange={(e) => setText(e.target.value)}
      />
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex justify-end gap-1.5">
        <Button size="sm" variant="ghost" disabled={pending} onClick={onClose}>
          Cancel
        </Button>
        <Button size="sm" variant="brand" disabled={pending || !text.trim()} onClick={() => void pair()}>
          {pending && <Loader2 className="animate-spin" />}
          Pair
        </Button>
      </div>
    </Modal>
  );
}

/**
 * A folder path on a remote host: its native picker would browse this
 * computer instead.
 */
export function HostPathDialog({ onDone }: { onDone(path: string | null): void }) {
  const [path, setPath] = useState("");
  return (
    <Modal label="Add a folder on the host" onClose={() => onDone(null)}>
      <p className="text-muted-foreground">The folder's absolute path on the host's computer.</p>
      <input
        className="bg-input/30 w-full rounded-md border px-2 py-1.5 font-mono text-xs"
        aria-label="Folder path on the host"
        placeholder="/Users/me/project"
        autoFocus
        value={path}
        onChange={(e) => setPath(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && path.trim()) onDone(path.trim());
        }}
      />
      <div className="flex justify-end gap-1.5">
        <Button size="sm" variant="ghost" onClick={() => onDone(null)}>
          Cancel
        </Button>
        <Button size="sm" variant="brand" disabled={!path.trim()} onClick={() => onDone(path.trim())}>
          Add
        </Button>
      </div>
    </Modal>
  );
}
