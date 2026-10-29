// The General tab's settings: what this computer's host does while nobody has
// the app open.

import { useEffect, useState } from "react";
import { reachability, type KeepAwakeMode, type KeepAwakeStatus, type Reachability } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import type { HostLoginStatus, NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import type { SettingMeta } from "@/settings/registry";
import { SettingRow, ToggleRow } from "@/settings/rows";

/** Whether the host on this computer starts at login. Hidden where it cannot. */
export function HostLoginToggle({ bridge, setting }: { bridge: NativeBridge; setting: SettingMeta }) {
  const login = bridge.hostLogin;
  const [status, setStatus] = useState<HostLoginStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    login?.status().then(setStatus, () => {});
  }, [login]);
  if (!login || !status?.supported) return null;
  return (
    <ToggleRow
      setting={setting}
      testId="host-login"
      checked={status.enabled}
      error={error}
      onChange={(enabled) => {
        setError(null);
        login.set(enabled).then(setStatus, (err) => setError(errorMessage(err)));
      }}
    />
  );
}

const MODES: { value: KeepAwakeMode; label: string }[] = [
  { value: "always", label: "Always (recommended)" },
  { value: "while_working", label: "Only while agents work" },
  { value: "off", label: "Never" },
];

const WARNINGS: Record<KeepAwakeStatus["warnings"][number], string> = {
  lid_close_will_sleep: "Closing the lid will still sleep this Mac",
  on_battery_will_drain: "Running on battery: this drains it",
};

/** What the host is doing about sleep right now, and the dot that goes with it. */
const REACHABILITY: Record<Exclude<Reachability, "unsupported">, { text: string; dot: string }> = {
  off: { text: "This Mac sleeps as usual", dot: "bg-faint" },
  low_battery: { text: "Will sleep: battery below 10%", dot: "bg-faint" },
  on_battery: { text: "Will sleep: on battery", dot: "bg-faint" },
  thermal: { text: "Will sleep: Mac is too hot", dot: "bg-faint" },
  held: { text: "Reachable: holding awake", dot: "bg-done" },
  while_working: { text: "Reachable while agents work", dot: "bg-faint" },
  not_held: { text: "Could not keep this Mac awake", dot: "bg-faint" },
};

/**
 * This computer's host keeps the Mac awake so phones can still reach it,
 * and says plainly when it will sleep anyway. Follows the host feed while open.
 */
export function KeepReachable({ bridge, setting }: { bridge: NativeBridge; setting: SettingMeta }) {
  const controller = useController();
  const fed = useHost((s) => s.keepAwake);
  const [status, setStatus] = useState<KeepAwakeStatus | null>(fed);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (fed) setStatus(fed);
  }, [fed]);
  useEffect(() => {
    // A Host that has not fed the status yet (or never will) is asked once.
    if (controller.store.getState().keepAwake) return;
    controller.client.request("host.keep_awake.status", {}).then(setStatus, () => {});
  }, [controller]);
  if (!status?.supported) return null;
  const send = (patch: Partial<{ mode: KeepAwakeMode; on_battery: boolean; lid_closed: boolean }>) => {
    setError(null);
    controller.client.request("host.keep_awake.set", patch).then(setStatus, (err) => setError(errorMessage(err)));
  };
  const now = REACHABILITY[reachability(status) as Exclude<Reachability, "unsupported">];
  return (
    <section className="flex flex-col gap-1 rounded-md border p-3" data-setting-id={setting.id}>
      <span className="font-medium">{setting.title}</span>
      <div className="flex flex-col gap-1" role="radiogroup" aria-label={setting.title}>
        {MODES.map((m) => (
          <button
            key={m.value}
            className="flex items-center gap-2 text-left"
            role="radio"
            aria-checked={status.mode === m.value}
            onClick={() => send({ mode: m.value })}
          >
            <span
              className={cn(
                "grid size-3.5 shrink-0 place-items-center rounded-full border",
                status.mode === m.value && "border-foreground",
              )}
            >
              {status.mode === m.value && <span className="bg-foreground size-1.5 rounded-full" />}
            </span>
            {m.label}
          </button>
        ))}
      </div>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          data-testid="keep-awake-battery"
          checked={status.on_battery}
          disabled={status.mode === "off"}
          onChange={(e) => send({ on_battery: e.target.checked })}
        />
        <span>Also on battery</span>
        <span className="text-muted-foreground">Stops below 10%</span>
      </label>
      <LidClosed
        status={status}
        bridge={bridge}
        onChange={(lid_closed) => send({ lid_closed })}
        onSetUp={() => controller.client.request("host.keep_awake.status", {}).then(setStatus, () => {})}
        onError={setError}
      />
      <p className="flex items-center gap-2" data-testid="keep-awake-status">
        <span className={cn("size-2 shrink-0 rounded-full", now.dot)} />
        {now.text}
      </p>
      {status.warnings.map((w) => (
        <p key={w} className="text-muted-foreground">
          {WARNINGS[w]}
        </p>
      ))}
      <p className="text-muted-foreground">The screen still turns off. Lock your Mac, don&apos;t log out.</p>
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/**
 * Keeping a closed laptop awake needs the power helper; until it runs, this
 * offers to set it up where the desktop app can.
 */
function LidClosed({
  status,
  bridge,
  onChange,
  onSetUp,
  onError,
}: {
  status: KeepAwakeStatus;
  bridge: NativeBridge;
  onChange: (enabled: boolean) => void;
  onSetUp: () => void;
  onError: (message: string) => void;
}) {
  const [approval, setApproval] = useState(false);
  const [busy, setBusy] = useState(false);
  const { support, enabled } = status.lid_closed;
  if (support === "available") {
    return (
      <div className="flex flex-col gap-1" data-testid="keep-awake-lid">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={enabled}
            disabled={status.mode === "off"}
            onChange={(e) => onChange(e.target.checked)}
          />
          <span>Stay awake with the lid closed</span>
        </label>
        <p className="text-muted-foreground">Don&apos;t put the Mac in a bag while this is on.</p>
      </div>
    );
  }
  const helper = bridge.lidHelper;
  if (support !== "needs_setup" || !helper) return null;
  const setUp = () => {
    setBusy(true);
    helper
      .setup()
      .then(
        (result) => {
          setApproval(result === "requires_approval");
          onSetUp();
        },
        (err) => onError(errorMessage(err)),
      )
      .finally(() => setBusy(false));
  };
  return (
    <div className="flex flex-col gap-1" data-testid="keep-awake-lid">
      <div className="flex items-center gap-2">
        <span className="flex-1">Stay awake with the lid closed</span>
        <Button size="xs" variant="outline" disabled={busy} onClick={setUp}>
          Set up
        </Button>
      </div>
      {approval && <p className="text-muted-foreground">Allow Zwiim in System Settings → Login Items</p>}
    </div>
  );
}
