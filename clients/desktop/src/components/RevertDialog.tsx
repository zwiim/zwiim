import { useState } from "react";
import { Loader2 } from "lucide-react";
import { revertRefusalText, type RevertChoice } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/host-context";

/**
 * Asks before a session goes back to before one of its messages. With `chat`
 * (the agent and Host can take the chat back, and the message has a reply
 * before it), the chat is reset and the files with it unless unchecked;
 * otherwise only the files are, and the copy says nothing about messages.
 * A refusal keeps it open and reads under the buttons.
 */
export function RevertDialog({
  chat,
  disabled,
  onRevert,
  onClose,
}: {
  chat: boolean;
  /** Offline: the reset waits. */
  disabled?: boolean;
  onRevert(choice: RevertChoice): Promise<void>;
  onClose(): void;
}) {
  const [files, setFiles] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const title = chat ? "Reset chat to here?" : "Reset files to here?";

  async function reset() {
    setBusy(true);
    setError(null);
    try {
      await onRevert(chat ? { files, chat: true } : { files: true, chat: false });
      onClose();
    } catch (e) {
      setError(revertRefusalText(e) ?? errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <Dialog className="max-w-md" aria-label={title} dismissible={!busy} onClose={onClose}>
      <div className="flex flex-col gap-2 px-5 pt-5">
        <h2 className="text-xl font-semibold tracking-tight">{title}</h2>
        <p className="text-muted-foreground text-sm leading-relaxed">
          {chat
            ? "Later messages will be removed."
            : "Files go back to how they were before this message. The chat stays as it is."}
        </p>
      </div>
      {chat && (
        <div className="flex flex-col gap-3 px-5 pt-4">
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={files} disabled={busy} onChange={(e) => setFiles(e.target.checked)} />
            Also reset files
          </label>
        </div>
      )}
      <div className="flex flex-col gap-2 p-5">
        <div className="flex justify-end gap-2">
          <Button variant="secondary" size="sm" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="destructive" size="sm" disabled={busy || disabled} onClick={() => void reset()}>
            {busy && <Loader2 className="animate-spin" />}
            {chat ? "Reset" : "Reset files"}
          </Button>
        </div>
        {error && (
          <p className="text-destructive text-xs" role="alert">
            {error}
          </p>
        )}
      </div>
    </Dialog>
  );
}
