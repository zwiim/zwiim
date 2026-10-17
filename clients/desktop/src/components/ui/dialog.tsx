import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/utils";

/** The exit animations' length (DESIGN.md "Motion": dialogs leave in 130ms). */
const EXIT_MS = 130;

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * A centered modal over the window: the scrim fades, the panel pops. Escape and
 * a click on the scrim close it unless `dismissible` is false (an action is
 * running). Focus moves in on open, Tab cycles inside, and focus goes back to
 * what had it once the dialog is gone. It renders into `body`, so no ancestor
 * stacking context or transform can trap it under the page. The parent unmounts it to close; a
 * still copy of it then plays the exit, so `onClose` stays immediate.
 */
export function Dialog({
  onClose,
  dismissible = true,
  className,
  children,
  ...panel
}: React.ComponentProps<"div"> & { onClose(): void; dismissible?: boolean }) {
  const overlay = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);
  // Read while rendering: by the first effect an autoFocus field already has focus.
  const [opener] = useState(() => (typeof document === "undefined" ? null : (document.activeElement as HTMLElement | null)));
  const latest = useRef({ onClose, dismissible });
  latest.current = { onClose, dismissible };

  useEffect(() => {
    if (!box.current?.contains(document.activeElement)) box.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && latest.current.dismissible) latest.current.onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useLayoutEffect(() => {
    const node = overlay.current;
    return () => {
      // A Strict Mode re-run keeps the node; only a real close removes it.
      queueMicrotask(() => {
        if (!node || node.isConnected) return;
        if (opener?.isConnected && (document.activeElement === document.body || document.activeElement === null)) {
          opener.focus();
        }
        const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches ?? true;
        if (reduce) return;
        const ghost = node.cloneNode(true) as HTMLElement;
        ghost.setAttribute("aria-hidden", "true");
        ghost.inert = true;
        ghost.style.pointerEvents = "none";
        ghost.querySelectorAll("[data-state]").forEach((el) => el.setAttribute("data-state", "closed"));
        ghost.setAttribute("data-state", "closed");
        document.body.appendChild(ghost);
        setTimeout(() => ghost.remove(), EXIT_MS);
      });
    };
  }, [opener]);

  return createPortal(
    <div
      ref={overlay}
      className="bg-scrim presence-fade fixed inset-0 z-50 grid place-items-center p-6"
      data-state="open"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget && dismissible) onClose();
      }}
    >
      <div
        ref={box}
        className={cn(
          "bg-popover text-foreground ring-border shadow-pop presence-pop flex w-full flex-col rounded-xl ring-1 focus:outline-none",
          className,
        )}
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        data-state="open"
        {...panel}
        onKeyDown={(e) => {
          panel.onKeyDown?.(e);
          if (e.key !== "Tab" || e.defaultPrevented) return;
          const items = [...(box.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])].filter(
            (el) => !el.hidden && el.getClientRects().length > 0,
          );
          const first = items[0];
          const last = items[items.length - 1];
          if (!first || !last) {
            e.preventDefault();
            return;
          }
          if (e.shiftKey && (document.activeElement === first || document.activeElement === box.current)) {
            e.preventDefault();
            last.focus();
          } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault();
            first.focus();
          }
        }}
      >
        {children}
      </div>
    </div>,
    document.body,
  );
}
