// A session's strict site list (design 13.2): when it has sites, the
// session's agent opens only those and its own dev servers. The person's own
// navigations are never limited.

import { useEffect, useRef, useState } from "react";
import { ShieldCheck, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";
import { POP, useDismiss } from "@/panels/BrowserPageUi";
import { cn } from "@/lib/utils";

export function BrowserSites({
  sessionId,
  disabled,
}: {
  sessionId: string;
  disabled: boolean;
}) {
  const controller = useController();
  const [sites, setSites] = useState<string[] | null>(null);
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, () => setOpen(false));

  useEffect(() => {
    let live = true;
    setSites(null);
    controller
      .browserSites(sessionId)
      .then((s) => live && setSites(s))
      .catch(() => live && setSites([]));
    return () => {
      live = false;
    };
  }, [controller, sessionId]);

  async function save(next: string[]) {
    setBusy(true);
    setError(null);
    try {
      setSites(await controller.setBrowserSites(sessionId, next));
      return true;
    } catch (e) {
      setError(errorMessage(e));
      return false;
    } finally {
      setBusy(false);
    }
  }

  const count = sites?.length ?? 0;
  return (
    <div ref={ref} className="relative shrink-0">
      <Button
        size={count ? "xs" : "icon-sm"}
        variant="ghost"
        title="Sites the agent may open"
        aria-expanded={open}
        disabled={disabled || sites === null}
        onClick={() => setOpen((o) => !o)}
      >
        <ShieldCheck />
        {count > 0 && <span className="tabular-nums">{count}</span>}
      </Button>
      {open && sites && (
        <div
          className={cn(
            POP,
            "top-full right-0 mt-1 flex w-72 flex-col gap-2 p-3",
          )}
          role="dialog"
          aria-label="Sites the agent may open"
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") setOpen(false);
          }}
        >
          <p className="text-ui font-medium">Sites the agent may open</p>
          <p className="text-muted-foreground text-xs">
            {count
              ? "This session's agent opens only these sites and its own dev servers. Your own browsing is not limited."
              : "Any site. Add one to keep this session's agent to a list of sites; its own dev servers stay allowed."}
          </p>
          {count > 0 && (
            <ul className="flex max-h-48 flex-col overflow-y-auto">
              {sites.map((site) => (
                <li
                  key={site}
                  className="hover:bg-muted flex items-center gap-2 rounded-md px-2 py-1"
                >
                  <span className="text-ui min-w-0 flex-1 truncate font-mono">
                    {site}
                  </span>
                  <button
                    className="text-muted-foreground hover:text-foreground disabled:opacity-50"
                    aria-label={`Remove ${site}`}
                    disabled={busy}
                    onClick={() => void save(sites.filter((s) => s !== site))}
                  >
                    <X className="size-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              const entry = draft.trim();
              if (!entry) return;
              void save([...sites, entry]).then((ok) => ok && setDraft(""));
            }}
          >
            <input
              autoFocus
              aria-label="Add a site"
              placeholder="example.com or *.example.com"
              className="border-border bg-input/30 text-ui h-8 min-w-0 flex-1 rounded-md border px-2 outline-none focus-visible:ring-2"
              value={draft}
              spellCheck={false}
              onChange={(e) => setDraft(e.target.value)}
            />
            <Button type="submit" size="sm" disabled={busy || !draft.trim()}>
              Add
            </Button>
          </form>
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
