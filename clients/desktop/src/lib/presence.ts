import { useEffect, useState } from "react";

/** Keeps a closing surface mounted for its exit animation (DESIGN.md "Motion": exits 130ms).
    Render while `mounted`; put `state` on `data-state` with the `presence-pop`/`presence-fade`
    utility. Under Reduce Motion the CSS exit is 1ms, so the extra frames are never seen. */
export function usePresence(open: boolean, exitMs = 130): { mounted: boolean; state: "open" | "closed" } {
  const [mounted, setMounted] = useState(open);
  useEffect(() => {
    if (open) {
      setMounted(true);
      return;
    }
    const t = setTimeout(() => setMounted(false), exitMs);
    return () => clearTimeout(t);
  }, [open, exitMs]);
  return { mounted: open || mounted, state: open ? "open" : "closed" };
}
