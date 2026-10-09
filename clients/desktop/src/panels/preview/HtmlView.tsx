import { useEffect, useState } from "react";
import { useController } from "@/lib/host-context";
import { closePreview, openPreview, previewUrl } from "@/lib/preview-host";
import { PreviewLoading } from "./PreviewState";

// A normal web page: scripts, forms, popups and downloads work. It may not
// navigate the app's window, and its origin is its own, never the app's.
const SANDBOX =
  "allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox allow-downloads allow-pointer-lock allow-presentation";

/** The page at its own origin. `reloadKey` changes only from the Reload button. */
export function HtmlView({
  workspaceId,
  path,
  reloadKey,
  onError,
}: {
  workspaceId: string;
  path: string;
  reloadKey: number;
  onError: (e: unknown) => void;
}) {
  const controller = useController();
  const [id, setId] = useState<string | null>(null);
  useEffect(() => {
    let gone = false;
    let opened: string | null = null;
    openPreview(controller.client, workspaceId).then(
      (got) => {
        opened = got;
        if (gone) void closePreview(got);
        else setId(got);
      },
      (e) => !gone && onError(e),
    );
    return () => {
      gone = true;
      if (opened) void closePreview(opened);
      setId(null);
    };
  }, [controller, workspaceId, onError]);
  if (!id) return <PreviewLoading />;
  return (
    <iframe
      key={reloadKey}
      title={path}
      className="min-h-0 flex-1 border-0 bg-white"
      src={previewUrl(id, path)}
      sandbox={SANDBOX}
      data-testid="html-frame"
    />
  );
}
