import { useCallback, useEffect, useState } from "react";
import { Copy, Loader2 } from "lucide-react";
import { renderSVG } from "uqr";
import type { CloudStatus, DeviceInfo, KeyPairingInvitation, LanStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { enableRemoteAccess } from "@/lib/remote";

/** While a pairing code shows, how often to look for the device it paired. */
const POLL_MS = 2000;

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "never");

/**
 * This computer's host: the devices it trusts, pairing another one, its link
 * to Zwiim Cloud and its local network access. Local authority only — a remote window never shows it.
 */
export function DevicesSettings({ bridge }: { bridge: NativeBridge }) {
  const controller = useController();
  const online = useOnline();
  const [devices, setDevices] = useState<DeviceInfo[] | null>(null);
  const [invite, setInvite] = useState<KeyPairingInvitation | null>(null);
  const [paired, setPaired] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const { devices } = await controller.client.request("device.list", {});
    setDevices(devices);
    return devices;
  }, [controller]);

  useEffect(() => {
    refresh().catch((e) => setError(errorMessage(e)));
  }, [refresh]);

  // A code is showing: watch for the device that uses it.
  useEffect(() => {
    if (!invite) return;
    const before = new Set((devices ?? []).filter((d) => d.status === "trusted").map((d) => d.device_id));
    const t = setInterval(() => {
      refresh().then(
        (now) => {
          const fresh = now.find((d) => d.status === "trusted" && !before.has(d.device_id));
          if (fresh) {
            setPaired(fresh.device_name);
            setInvite(null);
          }
        },
        () => {},
      );
    }, POLL_MS);
    return () => clearInterval(t);
    // Only a new invitation restarts the watch.
  }, [invite, refresh]);

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

  const pairDevice = () =>
    run("invite", async () => {
      setPaired(null);
      setInvite(await controller.client.request("device.pair_invite", {}));
    });

  const revoke = (d: DeviceInfo) =>
    run(`revoke:${d.device_id}`, async () => {
      await controller.client.request("device.revoke", { device_id: d.device_id });
      await refresh();
    });

  return (
    <>
      <RemoteAccess bridge={bridge} />
      <LanAccess />
      <div className="flex items-center">
        <p className="font-medium">Paired devices</p>
        <Button className="ml-auto" size="xs" variant="outline" disabled={!online || busy !== null} onClick={() => void pairDevice()}>
          Pair a device
        </Button>
      </div>
      {invite && <PairingCode invite={invite} onExpired={() => setInvite(null)} />}
      {paired && <p className="text-emerald-700 dark:text-emerald-400">Paired {paired}.</p>}
      {devices === null ? (
        <Loader2 className="text-muted-foreground size-4 animate-spin" />
      ) : devices.length === 0 ? (
        <p className="text-muted-foreground">No devices yet.</p>
      ) : (
        <ul className="flex flex-col divide-y rounded-md border" data-testid="devices">
          {devices.map((d) => (
            <li key={d.device_id} className="flex items-center gap-2 px-2.5 py-2">
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="truncate font-medium">{d.device_name}</span>
                <span className="text-muted-foreground truncate">
                  {d.status === "revoked"
                    ? `Revoked ${when(d.revoked_at)}`
                    : `Paired ${when(d.paired_at)} · last seen ${when(d.last_seen_at)}`}
                </span>
              </div>
              {d.status === "trusted" && (
                <Button
                  size="xs"
                  variant="destructive"
                  disabled={!online || busy !== null}
                  title="The device is disconnected now and cannot reconnect until paired again"
                  onClick={() => void revoke(d)}
                >
                  {busy === `revoke:${d.device_id}` ? "Revoking…" : "Revoke"}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

/**
 * The host's pairing QR: its id, its static key and a one-time code — no
 * address, nothing reusable. Another device scans it, or pastes its text.
 */
function PairingCode({ invite, onExpired }: { invite: KeyPairingInvitation; onExpired(): void }) {
  const text = JSON.stringify(invite);
  const expires = Date.parse(invite.expires_at);
  const [now, setNow] = useState(Date.now());
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const left = Math.max(0, Math.ceil((expires - now) / 1000));
  useEffect(() => {
    if (left === 0) onExpired();
  }, [left, onExpired]);
  return (
    <div className="flex flex-col items-center gap-2 rounded-md border p-3" data-testid="pairing-code">
      <div
        className="size-44 bg-white p-1 [&_svg]:size-full"
        role="img"
        aria-label="Pairing QR code"
        dangerouslySetInnerHTML={{ __html: renderSVG(text, { border: 1 }) }}
      />
      <p className="text-muted-foreground text-center">
        On your phone, open Zwiim and tap Scan pairing code. On another computer, choose Pair with a host and paste the
        text below. Expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}.
      </p>
      <div className="flex w-full items-center gap-1.5">
        <code className="bg-muted min-w-0 flex-1 truncate rounded px-1.5 py-1 text-[10px]" data-testid="pairing-text">
          {text}
        </code>
        <Button
          size="xs"
          variant="outline"
          onClick={() => void navigator.clipboard?.writeText(text).then(() => setCopied(true))}
        >
          <Copy /> {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
}

/** Whether other devices can reach this host through Zwiim Cloud, and turning it on. */
function RemoteAccess({ bridge }: { bridge: NativeBridge }) {
  const controller = useController();
  const online = useOnline();
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [account, setAccount] = useState<{ signedIn: boolean; cloudUrl: string | null } | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("cloud.status", {}).then(setStatus, (e) => setError(errorMessage(e)));
    bridge.accountStatus().then(
      (s) => setAccount({ signedIn: Boolean(s.account), cloudUrl: (bridge.remote && s.cloud_url) || null }),
      () => setAccount({ signedIn: false, cloudUrl: null }),
    );
  }, [controller, bridge]);

  async function enable() {
    const remote = bridge.remote;
    if (!remote || !account?.cloudUrl) return;
    setPending(true);
    setError(null);
    try {
      setStatus(await enableRemoteAccess(remote, account.cloudUrl, controller.client));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  }

  if (!status || !account) return null;
  if (!status.enrolled && !account.cloudUrl) return null;
  // Enabled on another Cloud than this app uses (e.g. its URL changed).
  const elsewhere = status.enrolled && account.signedIn && account.cloudUrl && status.cloud_url !== account.cloudUrl;
  const connection = {
    not_enrolled: "Off",
    connecting: "Connecting",
    online: "On",
    offline: "Offline — reconnecting",
  }[status.connection];
  return (
    <div className="flex flex-col gap-1.5 rounded-md border p-2.5" data-testid="remote-access">
      <div className="flex items-center gap-2">
        <span className="font-medium">Remote access</span>
        <span className="text-muted-foreground">{connection}</span>
        {!status.enrolled && account.signedIn && (
          <Button className="ml-auto" size="xs" variant="outline" disabled={pending || !online} onClick={() => void enable()}>
            {pending ? "Enabling…" : "Enable"}
          </Button>
        )}
        {elsewhere && (
          <Button className="ml-auto" size="xs" variant="outline" disabled={pending || !online} onClick={() => void enable()}>
            {pending ? "Moving…" : "Move to this Cloud"}
          </Button>
        )}
      </div>
      {elsewhere && (
        <p className="text-muted-foreground break-words">
          Enabled on {status.cloud_url}; this app uses {account.cloudUrl}.
        </p>
      )}
      {!status.enrolled && !account.signedIn && (
        <p className="text-muted-foreground">Sign in to let your other devices reach this host.</p>
      )}
      {status.enrolled && status.last_error && status.connection !== "online" && (
        <p className="text-muted-foreground break-words">{status.last_error}</p>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * Whether paired devices on this network can connect to this host directly,
 * without Zwiim Cloud. They are the same devices, with the same pairing, as
 * through the Cloud; nothing else is accepted.
 */
function LanAccess() {
  const controller = useController();
  const online = useOnline();
  const [status, setStatus] = useState<LanStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // A host without local network access says nothing here.
    controller.client.request("lan.status", {}).then(setStatus, () => setStatus(null));
  }, [controller]);

  async function set(enabled: boolean) {
    setPending(true);
    setError(null);
    try {
      setStatus(await controller.client.request("lan.set", { enabled }));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  }

  if (!status) return null;
  const state = !status.enabled ? "Off" : status.listening ? `On · port ${status.port}` : "Not listening";
  return (
    <div className="flex flex-col gap-1.5 rounded-md border p-2.5" data-testid="lan-access">
      <div className="flex items-center gap-2">
        <span className="font-medium">Local network access</span>
        <span className="text-muted-foreground">{state}</span>
        <Button
          className="ml-auto"
          size="xs"
          variant="outline"
          disabled={pending || !online}
          onClick={() => void set(!status.enabled)}
        >
          {pending ? (status.enabled ? "Turning off…" : "Turning on…") : status.enabled ? "Turn off" : "Turn on"}
        </Button>
      </div>
      <p className="text-muted-foreground">
        Paired devices on this network connect directly, without Zwiim Cloud.
      </p>
      {status.enabled && status.last_error && (
        <p className="text-muted-foreground break-words">{status.last_error}</p>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
