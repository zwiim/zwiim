import { useEffect, useState } from "react";
import { Bot, Info, Monitor, Moon, Network, Settings2, Smartphone, Sun, UserRound, X } from "lucide-react";
import type { KeepAwakeMode, KeepAwakeStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import type { AccountState } from "@/lib/account";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, useController } from "@/lib/host-context";
import { rememberTheme, theme, type Theme } from "@/lib/prefs";
import { applyTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { AboutUpdates, HostLoginToggle } from "@/panels/About";
import { AccountPanel } from "@/panels/Account";
import { AgentsSettings } from "@/panels/Agents";
import { ConnectionModeChooser, type RemoteLink } from "@/panels/ConnectionStatus";
import { DevicesSettings } from "@/panels/Devices";

export type SettingsTab = "general" | "account" | "agents" | "connection" | "devices" | "about";

interface TabInfo {
  id: SettingsTab;
  label: string;
  icon: typeof Info;
  group: "Settings" | "Host";
}

/**
 * Everything about this app and the host it shows, one pane per tab. What
 * belongs to this computer's host only (its devices, starting at login) is
 * not offered for a remote host; how a remote host is reached is offered for
 * it alone.
 */
export function SettingsDialog({
  bridge,
  remote,
  account,
  link,
  updateHint,
  tab,
  onTab,
  onClose,
}: {
  bridge: NativeBridge;
  remote: boolean;
  account: AccountState;
  link?: RemoteLink;
  updateHint: boolean;
  tab: SettingsTab;
  onTab(tab: SettingsTab): void;
  onClose(): void;
}) {
  const tabs: TabInfo[] = [
    { id: "general", label: "General", icon: Settings2, group: "Settings" },
    { id: "account", label: "Account", icon: UserRound, group: "Settings" },
    { id: "agents", label: "Agents", icon: Bot, group: "Host" },
    ...(remote && link ? [{ id: "connection", label: "Connection", icon: Network, group: "Host" } as const] : []),
    ...(!remote ? [{ id: "devices", label: "Devices", icon: Smartphone, group: "Host" } as const] : []),
    { id: "about", label: "About & updates", icon: Info, group: "Host" },
  ];
  const current = tabs.find((t) => t.id === tab) ?? tabs[0]!;

  return (
    <Dialog className="h-[min(640px,100%)] max-w-3xl flex-row overflow-hidden" aria-label="Settings" onClose={onClose}>
      <nav className="bg-sidebar flex w-52 shrink-0 flex-col gap-0.5 p-2" aria-label="Settings sections">
        {(["Settings", "Host"] as const).map((group) => (
          <div key={group} className="flex flex-col gap-0.5">
            <span className="text-muted-foreground px-2 pt-3 pb-1 text-2xs font-semibold tracking-wide uppercase">{group}</span>
            {tabs
              .filter((t) => t.group === group)
              .map((t) => (
                <button
                  key={t.id}
                  className={cn(
                    "flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-ui transition-colors",
                    t.id === current.id
                      ? "bg-sidebar-accent text-foreground"
                      : "text-muted-foreground hover:text-foreground hover:bg-sidebar-accent/60",
                  )}
                  aria-current={t.id === current.id ? "page" : undefined}
                  onClick={() => onTab(t.id)}
                >
                  <t.icon className="size-4 shrink-0" />
                  <span className="flex-1">{t.label}</span>
                  {t.id === "about" && updateHint && (
                    <span className="bg-brand size-2 rounded-full" aria-label="Update available" />
                  )}
                </button>
              ))}
          </div>
        ))}
      </nav>
      <div className="relative flex min-w-0 flex-1 flex-col">
        <Button className="absolute top-3 right-3" size="icon-sm" variant="ghost" title="Close" onClick={onClose}>
          <X />
        </Button>
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-7 py-6 text-xs" data-testid={`settings-${current.id}`}>
          <h2 className="mb-1 text-lg font-semibold tracking-tight">{current.label}</h2>
          {current.id === "general" && <General bridge={bridge} remote={remote} />}
          {current.id === "account" && <AccountPanel state={account} />}
          {current.id === "agents" && <AgentsSettings remote={remote} />}
          {current.id === "connection" && link && (
            <>
              <p className="text-muted-foreground">How this computer reaches the host.</p>
              <ConnectionModeChooser link={link} />
            </>
          )}
          {current.id === "devices" && <DevicesSettings bridge={bridge} />}
          {current.id === "about" && <AboutUpdates bridge={bridge} remote={remote} />}
        </div>
      </div>
    </Dialog>
  );
}

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

function General({ bridge, remote }: { bridge: NativeBridge; remote: boolean }) {
  const [current, setCurrent] = useState(theme);
  return (
    <>
      <div className="flex items-center gap-3 border-b pb-3">
        <span className="text-ui flex-1">Theme</span>
        <div className="bg-muted flex gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label="Theme">
          {THEMES.map((t) => (
            <button
              key={t.value}
              className={cn(
                "grid size-7 place-items-center rounded-md transition-colors",
                current === t.value ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
              )}
              role="radio"
              aria-checked={current === t.value}
              aria-label={t.label}
              title={t.label}
              onClick={() => {
                rememberTheme(t.value);
                applyTheme(t.value);
                setCurrent(t.value);
              }}
            >
              <t.icon className="size-3.5" />
            </button>
          ))}
        </div>
      </div>
      {!remote && <HostLoginToggle bridge={bridge} />}
      {!remote && <KeepReachable />}
    </>
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
function reachability(s: KeepAwakeStatus): { text: string; dot: string } {
  if (s.mode === "off") return { text: "This Mac sleeps as usual", dot: "bg-faint" };
  // While working, the host blocks only once a turn starts; say so before.
  const battery = s.power.source === "battery";
  const percent = s.power.battery_percent;
  const onBattery = s.blocked_by === "on_battery" || (battery && !s.on_battery);
  const lowBattery = s.blocked_by === "low_battery" || (battery && percent !== null && percent < 10);
  // Same order as the host: the 10% floor applies whatever the checkbox.
  if (lowBattery) return { text: "Will sleep: battery below 10%", dot: "bg-faint" };
  if (onBattery) return { text: "Will sleep: on battery", dot: "bg-faint" };
  if (s.blocked_by === "thermal") return { text: "Will sleep: Mac is too hot", dot: "bg-faint" };
  if (s.active) return { text: "Reachable: holding awake", dot: "bg-done" };
  if (s.mode === "while_working") return { text: "Reachable while agents work", dot: "bg-faint" };
  // Always, nothing in the way, yet no hold: the host tried and could not.
  return { text: "Could not keep this Mac awake", dot: "bg-faint" };
}

/**
 * This computer's host keeps the Mac awake so phones can still reach it,
 * and says plainly when it will sleep anyway. Follows the host while open.
 */
function KeepReachable() {
  const controller = useController();
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const load = () => controller.client.request("host.keep_awake.status", {}).then(setStatus, () => {});
    load();
    const timer = setInterval(load, 15000);
    return () => clearInterval(timer);
  }, [controller]);
  if (!status?.supported) return null;
  const send = (patch: Partial<{ mode: KeepAwakeMode; on_battery: boolean }>) => {
    setError(null);
    controller.client.request("host.keep_awake.set", patch).then(setStatus, (err) => setError(errorMessage(err)));
  };
  const now = reachability(status);
  return (
    <section className="flex flex-col gap-1 rounded-md border p-3">
      <span className="font-medium">Keep this Mac reachable</span>
      <div className="flex flex-col gap-1" role="radiogroup" aria-label="Keep this Mac reachable">
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
