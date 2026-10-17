import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import {
  Check,
  ChevronsUpDown,
  Info,
  Loader2,
  LogIn,
  LogOut,
  RefreshCw,
  Settings,
  ShieldAlert,
  Smartphone,
  X,
} from "lucide-react";
import type { AccountState } from "@/lib/account";
import { errorMessage } from "@/lib/host-context";
import { usePresence } from "@/lib/presence";
import { cn } from "@/lib/utils";
import { Avatar } from "@/panels/Account";
import type { SettingsTab } from "@/panels/Settings";

const itemClass =
  "hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-ui transition-colors disabled:opacity-50";

/** Arrow Up/Down, Home and End move focus between a menu's enabled items (the ARIA menu pattern). */
export function moveMenuFocus(e: ReactKeyboardEvent<HTMLElement>) {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const items = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)'));
  if (items.length === 0) return;
  e.preventDefault();
  const i = items.indexOf(document.activeElement as HTMLElement);
  const next =
    e.key === "Home"
      ? 0
      : e.key === "End"
        ? items.length - 1
        : e.key === "ArrowDown"
          ? (i + 1) % items.length
          : (i <= 0 ? items.length : i) - 1;
  items[next].focus();
}

/**
 * The sidebar's footer: who is signed in, and a menu up from it to settings,
 * this host's devices, versions, checking for updates and signing in or out.
 * Compact, it is the avatar alone on the collapsed rail, its menu opening rightward.
 */
export function ControlCenter({
  compact = false,
  account,
  remote,
  updateHint,
  onSettings,
  onCheckUpdates,
}: {
  compact?: boolean;
  account: AccountState;
  remote: boolean;
  updateHint: boolean;
  onSettings(tab: SettingsTab): void;
  /** Checks now; true when an update card now wants the user. */
  onCheckUpdates(): Promise<boolean>;
}) {
  const [open, setOpen] = useState(false);
  const [check, setCheck] = useState<"idle" | "checking" | "current">("idle");
  const [checkError, setCheckError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const presence = usePresence(open);
  const { status, pending } = account;
  const email = account.account?.email ?? null;
  const name = account.account ? (email ?? "Signed in") : "Local mode";

  useEffect(() => {
    if (open) return;
    setCheck("idle");
    setCheckError(null);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    menu.current?.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const onDown = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setOpen(false);
      trigger.current?.focus();
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const go = (tab: SettingsTab) => {
    setOpen(false);
    onSettings(tab);
  };

  async function checkUpdates() {
    setCheck("checking");
    setCheckError(null);
    try {
      // Something to update: the card below says what; otherwise say so here.
      if (await onCheckUpdates()) setOpen(false);
      else setCheck("current");
    } catch (e) {
      setCheck("idle");
      setCheckError(errorMessage(e));
    }
  }

  return (
    <div ref={root} className="relative" data-testid="control-center">
      {presence.mounted && (
        <div
          ref={menu}
          className={cn(
            "bg-card text-card-foreground ring-border presence-pop shadow-pop absolute z-30 flex flex-col gap-px rounded-xl p-1.5 ring-1",
            compact
              ? "bottom-0 left-full ml-2.5 w-64 origin-bottom-left"
              : "right-0 bottom-full left-0 mb-1.5 origin-bottom",
          )}
          role="menu"
          aria-label="Account and settings"
          data-state={presence.state}
          aria-hidden={!open || undefined}
          inert={!open}
          onKeyDown={moveMenuFocus}
        >
          <p className="text-muted-foreground truncate px-2.5 pt-1 pb-1.5 text-xs" title={email ?? undefined}>
            {account.account ? (email ?? "Signed in") : "Not signed in"}
          </p>
          <button className={itemClass} role="menuitem" onClick={() => go("general")}>
            <Settings className="text-muted-foreground size-4" />
            <span className="flex-1">Settings</span>
            <kbd className="text-muted-foreground font-sans text-2xs">⌘,</kbd>
          </button>
          {!remote && (
            <button className={itemClass} role="menuitem" onClick={() => go("devices")}>
              <Smartphone className="text-muted-foreground size-4" />
              Devices
            </button>
          )}
          <button className={itemClass} role="menuitem" onClick={() => go("about")}>
            <Info className="text-muted-foreground size-4" />
            <span className="flex-1">About & updates</span>
            {updateHint && <span className="bg-brand size-2 rounded-full" aria-label="Update available" />}
          </button>
          <button
            className={itemClass}
            role="menuitem"
            disabled={check === "checking"}
            onClick={() => void checkUpdates()}
          >
            {check === "checking" ? (
              <Loader2 className="text-muted-foreground size-4 animate-spin" />
            ) : check === "current" ? (
              <Check className="text-muted-foreground size-4" />
            ) : (
              <RefreshCw className="text-muted-foreground size-4" />
            )}
            {check === "checking" ? "Checking for updates…" : check === "current" ? "Up to date" : "Check for updates"}
          </button>
          {checkError && (
            <p className="text-destructive px-2.5 py-1 text-xs break-words" role="alert">
              {checkError}
            </p>
          )}
          {account.error && (
            <p className="text-destructive px-2.5 py-1 text-xs break-words" role="alert">
              {account.error}
            </p>
          )}
          {(account.account || status?.available) && <div className="bg-border mx-2.5 my-1 h-px" role="separator" />}
          {account.account ? (
            <button
              className={itemClass}
              role="menuitem"
              title="Forget this account on this computer. The host, its agents and sessions keep running."
              onClick={() => {
                setOpen(false);
                void account.signOut();
              }}
            >
              <LogOut className="text-muted-foreground size-4" />
              Sign out
            </button>
          ) : (
            status?.available &&
            (pending ? (
              <button className={itemClass} role="menuitem" onClick={() => void account.cancelSignIn()}>
                <X className="text-muted-foreground size-4" />
                Cancel sign-in
              </button>
            ) : (
              <button className={itemClass} role="menuitem" onClick={() => void account.signIn()}>
                <LogIn className="text-muted-foreground size-4" />
                Sign in
              </button>
            ))
          )}
        </div>
      )}
      {compact ? (
        <button
          ref={trigger}
          className={cn(
            "hover:ring-sidebar-accent relative flex size-7 rounded-full ring-2 ring-transparent transition-shadow",
            open && "ring-sidebar-accent",
          )}
          aria-haspopup="menu"
          aria-expanded={open}
          title={`${name} · Account and settings`}
          onClick={() => setOpen((o) => !o)}
        >
          <Avatar email={email} />
          {updateHint && (
            <span
              className="bg-brand ring-sidebar absolute -top-px -right-px size-2 rounded-full ring-2"
              aria-label="Update available"
            />
          )}
        </button>
      ) : (
        <button
          ref={trigger}
          className={cn(
            "hover:bg-sidebar-accent flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors",
            open && "bg-sidebar-accent",
          )}
          aria-haspopup="menu"
          aria-expanded={open}
          title="Account and settings"
          onClick={() => setOpen((o) => !o)}
        >
          <Avatar email={email} />
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="text-ui truncate font-medium">{name}</span>
            {pending && (
              <span className="text-muted-foreground text-2xs flex items-center gap-1">
                <Loader2 className="size-3 animate-spin" /> Finish signing in in your browser…
              </span>
            )}
          </span>
          {account.ownership.kind === "mismatch" && (
            <ShieldAlert className="text-brand size-3.5 shrink-0" aria-label="This host belongs to another account" />
          )}
          {updateHint && <span className="bg-brand size-2 shrink-0 rounded-full" aria-label="Update available" />}
          <ChevronsUpDown className="text-muted-foreground size-3.5 shrink-0" />
        </button>
      )}
    </div>
  );
}
