import { useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";

/** Asks the person to accept the risk of automating sites before the first run on them. */
export function StartDialog({
  name,
  sites,
  onStart,
  onClose,
}: {
  name: string;
  sites: string[];
  onStart(): Promise<void>;
  onClose(): void;
}) {
  const [busy, setBusy] = useState(false);
  const title = `Start ${name}?`;

  async function start() {
    setBusy(true);
    try {
      await onStart();
    } finally {
      onClose();
    }
  }

  return (
    <Dialog className="max-w-md" aria-label={title} dismissible={!busy} onClose={onClose}>
      <div className="flex flex-col gap-2 px-5 pt-5">
        <h2 className="text-xl font-semibold tracking-tight">{title}</h2>
        <p className="text-muted-foreground text-sm leading-relaxed">
          It works on {sites.join(", ")}. Sites can limit or block accounts that automate; Zwiim keeps a human pace to
          lower that risk.
        </p>
      </div>
      <div className="flex justify-end gap-2 p-5">
        <Button variant="secondary" size="sm" disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button variant="brand" size="sm" disabled={busy} onClick={() => void start()}>
          {busy && <Loader2 className="animate-spin" />}I accept the risk, start
        </Button>
      </div>
    </Dialog>
  );
}
