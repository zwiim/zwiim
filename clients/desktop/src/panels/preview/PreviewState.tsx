import { ExternalLink, Loader2 } from "lucide-react";
import { previewErrorMessage } from "@zwiim/preview";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/host-context";

/** Why a preview failed, with the way out: its own app. */
export function PreviewFailure({ error, name, opening, onOpen }: { error: unknown; name: string; opening: boolean; onOpen: () => void }) {
  return (
    <div className="flex flex-col items-start gap-3 p-4">
      <p className="text-destructive text-xs" role="alert" data-testid="file-error">
        {previewErrorMessage(error, name) ?? errorMessage(error)}
      </p>
      <Button size="sm" variant="outline" disabled={opening} onClick={onOpen}>
        {opening ? <Loader2 className="animate-spin" /> : <ExternalLink />} Open in default app
      </Button>
    </div>
  );
}

export function PreviewLoading() {
  return (
    <p className="text-muted-foreground flex items-center gap-1.5 p-4 text-xs">
      <Loader2 className="size-3 animate-spin" /> Opening…
    </p>
  );
}
