import { useEffect, useState } from "react";
import { Bot, Info, Monitor, Moon, Network, Settings2, Smartphone, Sun, UserRound, X } from "lucide-react";
import type { KeepAwakeStatus } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
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

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/50 p-6"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="bg-card text-card-foreground ring-border flex h-[min(640px,100%)] w-full max-w-3xl overflow-hidden rounded-xl shadow-2xl ring-1"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
      >
        <nav className="bg-sidebar flex w-52 shrink-0 flex-col gap-0.5 p-2" aria-label="Settings sections">
          {(["Settings", "Host"] as const).map((group) => (
            <div key={group} className="flex flex-col gap-0.5">
              <span className="text-muted-foreground px-2.5 pt-3 pb-1 text-[11px] font-medium">{group}</span>
              {tabs
                .filter((t) => t.group === group)
                .map((t) => (
                  <button
                    key={t.id}
                    className={cn(
                      "flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-[13px] transition-colors",
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
                      <span className="bg-brand size-1.5 rounded-full" aria-label="Update available" />
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
            <h2 className="mb-1 text-base font-semibold tracking-tight">{current.label}</h2>
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
      </div>
    </div>
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
        <span className="flex-1 text-[13px]">Theme</span>
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
      {!remote && <KeepAwakeToggle />}
    </>
  );
}

/** This computer's host keeps the Mac awake while an agent works. */
function KeepAwakeToggle() {
  const controller = useController();
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    controller.client.request("host.keep_awake.status", {}).then(setStatus, () => {});
  }, [controller]);
  if (!status?.supported) return null;
  return (
    <section className="flex flex-col gap-1 rounded-md border p-2.5">
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          data-testid="keep-awake"
          checked={status.enabled}
          onChange={(e) => {
            setError(null);
            controller.client
              .request("host.keep_awake.set", { enabled: e.target.checked })
              .then(setStatus, (err) => setError(errorMessage(err)));
          }}
        />
        <span className="font-medium">Keep this Mac awake while agents work</span>
      </label>
      <p className="text-muted-foreground">
        So long tasks finish while you are away. The screen can still turn off. Stops on battery below 10%.
      </p>
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
