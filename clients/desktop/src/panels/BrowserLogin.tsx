// "Keep me signed in for this workspace" (browser design 6.3): the person
// picks sites this session is signed in to, and the Host keeps their cookies,
// sealed, for every new session in the workspace. Only site names reach this
// window; the cookies stay on the Host.

import { useEffect, useRef, useState } from "react";
import { KeyRound, X } from "lucide-react";
import type { BrowserLoginSites } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";
import { POP, useDismiss } from "@/panels/BrowserPageUi";
import { cn } from "@/lib/utils";

export function BrowserLogin({ sessionId, disabled }: { sessionId: string; disabled: boolean }) {
  const controller = useController();
  const [sites, setSites] = useState<BrowserLoginSites | null>(null);
  const [open, setOpen] = useState(false);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, () => setOpen(false));

  // Read again on each open: the session signs in to sites as it goes.
  useEffect(() => {
    let live = true;
    setError(null);
    controller
      .browserLogin(sessionId)
      .then((s) => live && setSites(s))
      .catch((e) => {
        if (!live) return;
        setSites({ available: [], saved: [] });
        if (open) setError(errorMessage(e));
      });
    return () => {
      live = false;
    };
  }, [controller, sessionId, open]);

  async function act(f: () => Promise<string[]>) {
    setBusy(true);
    setError(null);
    try {
      const saved = await f();
      setSites((s) => s && { ...s, saved });
      setChosen(new Set());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const saved = sites?.saved ?? [];
  const fresh = (sites?.available ?? []).filter((s) => !saved.includes(s));
  const toggle = (site: string) =>
    setChosen((c) => {
      const next = new Set(c);
      if (!next.delete(site)) next.add(site);
      return next;
    });

  return (
    <div ref={ref} className="relative shrink-0">
      <Button
        size={saved.length ? "xs" : "icon-sm"}
        variant="ghost"
        title="Sign-ins kept for this workspace"
        aria-expanded={open}
        disabled={disabled || sites === null}
        onClick={() => setOpen((o) => !o)}
      >
        <KeyRound />
        {saved.length > 0 && <span className="tabular-nums">{saved.length}</span>}
      </Button>
      {open && sites && (
        <div
          className={cn(POP, "top-full right-0 mt-1 flex w-72 flex-col gap-2 p-3")}
          role="dialog"
          aria-label="Sign-ins kept for this workspace"
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") setOpen(false);
          }}
        >
          <p className="text-ui font-medium">Keep me signed in for this workspace</p>
          <p className="text-muted-foreground text-xs">
            New sessions in this workspace start signed in to the sites kept here. Zwiim keeps their cookies encrypted on
            this computer.
          </p>
          {saved.length > 0 && (
            <ul className="flex max-h-40 flex-col overflow-y-auto" aria-label="Kept">
              {saved.map((site) => (
                <li key={site} className="hover:bg-muted flex items-center gap-2 rounded-md px-2 py-1">
                  <span className="text-ui min-w-0 flex-1 truncate font-mono">{site}</span>
                  <button
                    className="text-muted-foreground hover:text-foreground disabled:opacity-50"
                    aria-label={`Forget ${site}`}
                    disabled={busy}
                    onClick={() => void act(() => controller.forgetBrowserLogin(sessionId, [site]))}
                  >
                    <X className="size-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          {fresh.length > 0 ? (
            <>
              <p className="text-muted-foreground text-xs">This session is signed in to:</p>
              <ul className="flex max-h-40 flex-col overflow-y-auto">
                {fresh.map((site) => (
                  <li key={site}>
                    <label className="hover:bg-muted flex items-center gap-2 rounded-md px-2 py-1">
                      <input type="checkbox" checked={chosen.has(site)} disabled={busy} onChange={() => toggle(site)} />
                      <span className="text-ui min-w-0 flex-1 truncate font-mono">{site}</span>
                    </label>
                  </li>
                ))}
              </ul>
              <Button
                size="sm"
                disabled={busy || chosen.size === 0}
                onClick={() => void act(() => controller.saveBrowserLogin(sessionId, [...chosen]))}
              >
                Keep signed in
              </Button>
            </>
          ) : (
            saved.length === 0 && (
              <p className="text-muted-foreground text-xs">Sign in to a site in this session's tab, then keep it here.</p>
            )
          )}
          {error && (
            <p className="text-destructive text-xs" role="alert">
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
