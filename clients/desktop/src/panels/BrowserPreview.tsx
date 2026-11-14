// Mobile preview (browser design 7.7): show the tab as a phone or tablet
// shows it. The Host gives the page the device's screen, touch and user
// agent; the view letterboxes the frames like any other page.

import { useRef, useState } from "react";
import { Check, Smartphone } from "lucide-react";
import { BROWSER_PREVIEWS, type BrowserPreviewId, type BrowserTab } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { ITEM, POP, useDismiss } from "@/panels/BrowserPageUi";
import { cn } from "@/lib/utils";

const OPTIONS: { id: BrowserPreviewId | null; name: string; size: string }[] = [
  { id: null, name: "Desktop", size: "" },
  ...BROWSER_PREVIEWS.map((p) => ({ id: p.id, name: p.name, size: `${p.width} × ${p.height}` })),
];

export function BrowserPreview({
  tab,
  disabled,
  onChoose,
}: {
  tab: BrowserTab;
  disabled: boolean;
  onChoose(preview: BrowserPreviewId | null): void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, () => setOpen(false));
  const chosen = tab.preview ?? null;
  const current = BROWSER_PREVIEWS.find((p) => p.id === chosen);
  const choose = (preview: BrowserPreviewId | null) => {
    setOpen(false);
    if (preview !== chosen) onChoose(preview);
  };

  return (
    <div ref={ref} className="relative shrink-0">
      <Button
        type="button"
        size="icon-sm"
        variant={current ? "secondary" : "ghost"}
        title={current ? `Previewing as ${current.name}` : "Preview as a phone or tablet"}
        aria-expanded={open}
        aria-pressed={!!current}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        <Smartphone />
      </Button>
      {open && (
        <div
          className={cn(POP, "top-full right-0 mt-1 w-56")}
          role="menu"
          aria-label="Preview as"
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") setOpen(false);
          }}
        >
          {OPTIONS.map((o) => (
            <button
              key={o.id ?? "desktop"}
              type="button"
              role="menuitemradio"
              aria-checked={chosen === o.id}
              className={ITEM}
              onClick={() => choose(o.id)}
            >
              <Check className={cn("size-3.5", chosen === o.id ? "opacity-100" : "opacity-0")} />
              <span className="flex-1">{o.name}</span>
              {o.size && <span className="text-muted-foreground text-xs tabular-nums">{o.size}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
