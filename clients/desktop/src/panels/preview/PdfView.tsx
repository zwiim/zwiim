import { Minus, MoveHorizontal, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { openPdfSource, renderPdf, type PdfAssets, type PdfHandle } from "@zwiim/preview/pdf";
import { Button } from "@/components/ui/button";
import { useController } from "@/lib/host-context";
import { PreviewLoading } from "./PreviewState";

/** pdf.js's worker and data, copied into `public/pdfjs` at build. */
export const PDF_ASSETS: PdfAssets = {
  workerSrc: "/pdfjs/pdf.worker.min.mjs",
  cMapUrl: "/pdfjs/cmaps/",
  standardFontDataUrl: "/pdfjs/standard_fonts/",
  wasmUrl: "/pdfjs/wasm/",
};

/** A PDF read in ranges as its pages scroll into view, with zoom. */
export function PdfView({ workspaceId, path, onError }: { workspaceId: string; path: string; onError: (e: unknown) => void }) {
  const controller = useController();
  const box = useRef<HTMLDivElement>(null);
  const [handle, setHandle] = useState<PdfHandle | null>(null);
  useEffect(() => {
    let gone = false;
    let current: PdfHandle | null = null;
    (async () => {
      try {
        const source = await openPdfSource(controller.client, workspaceId, path);
        if (gone || !box.current) return;
        current = await renderPdf(box.current, source, PDF_ASSETS, (e) => !gone && onError(e));
        if (gone) current.destroy();
        else setHandle(current);
      } catch (e) {
        if (!gone) onError(e);
      }
    })();
    return () => {
      gone = true;
      current?.destroy();
      setHandle(null);
    };
  }, [controller, workspaceId, path, onError]);
  return (
    <div className="relative min-h-0 flex-1">
      <div className="absolute top-2 right-4 z-10 flex gap-1">
        <Button size="icon-sm" variant="outline" title="Zoom out" aria-label="Zoom out" disabled={!handle} onClick={() => handle?.zoomOut()}>
          <Minus />
        </Button>
        <Button size="icon-sm" variant="outline" title="Zoom in" aria-label="Zoom in" disabled={!handle} onClick={() => handle?.zoomIn()}>
          <Plus />
        </Button>
        <Button size="icon-sm" variant="outline" title="Fit width" aria-label="Fit width" disabled={!handle} onClick={() => handle?.fitWidth()}>
          <MoveHorizontal />
        </Button>
      </div>
      {!handle && <PreviewLoading />}
      <div ref={box} className="absolute inset-0 overflow-auto" data-testid="pdf-view" />
    </div>
  );
}
