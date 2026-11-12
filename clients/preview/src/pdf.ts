import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import { EventBus, LinkTarget, PDFFindController, PDFLinkService, PDFViewer } from "pdfjs-dist/legacy/web/pdf_viewer.mjs";
import "pdfjs-dist/legacy/web/pdf_viewer.css";
import type { FindOptions } from "./find";
import { pdfDocumentParams, type PdfAssets, type PdfSource } from "./pdf-source";

export * from "./pdf-source";

export interface PdfHandle {
  zoomIn(): void;
  zoomOut(): void;
  fitWidth(): void;
  /**
   * Finds `query` across the pages, marking every match. With `step`, moves to
   * the next or previous match of the same search instead of starting over.
   */
  find(query: string, options: FindOptions, step?: "next" | "previous"): void;
  closeFind(): void;
  /** Called as matches are counted: the current one (from 1, 0 for none) and the total. */
  onFindCount(listener: ((current: number, total: number) => void) | null): void;
  destroy(): void;
}

/** Continuous-scroll PDF viewer in `container` (absolutely positioned, scrollable). */
export async function renderPdf(
  container: HTMLDivElement,
  source: PdfSource,
  assets: PdfAssets,
  onError?: (e: unknown) => void,
): Promise<PdfHandle> {
  pdfjs.GlobalWorkerOptions.workerSrc = assets.workerSrc;
  const { kind, params } = pdfDocumentParams(source, assets);
  let fail!: (e: unknown) => void;
  const failed = new Promise<never>((_, reject) => (fail = reject));
  let loaded = false;
  let range: pdfjs.PDFDataRangeTransport | undefined;
  if (kind === "range" && "read" in source) {
    range = new pdfjs.PDFDataRangeTransport(source.size, source.initial);
    range.requestDataRange = (begin: number, end: number) => {
      source.read(begin, end).then(
        (bytes) => range!.onDataRange(begin, bytes),
        (e) => (loaded ? onError?.(e) : fail(e)),
      );
    };
  }
  const task = pdfjs.getDocument(range ? { ...params, range } : params);
  const doc = await Promise.race([task.promise, failed]).catch(async (e) => {
    await task.destroy();
    throw e;
  });
  loaded = true;

  container.replaceChildren();
  const pages = document.createElement("div");
  pages.className = "pdfViewer";
  container.append(pages);
  const eventBus = new EventBus();
  const linkService = new PDFLinkService({ eventBus, externalLinkTarget: LinkTarget.BLANK });
  const findController = new PDFFindController({ eventBus, linkService, updateMatchesCountOnProgress: true });
  const viewer = new PDFViewer({ container, viewer: pages, eventBus, linkService, findController, removePageBorders: false });
  linkService.setViewer(viewer);
  let counted: ((current: number, total: number) => void) | null = null;
  const count = ({ matchesCount }: { matchesCount?: { current: number; total: number } }) =>
    matchesCount && counted?.(matchesCount.current, matchesCount.total);
  eventBus.on("updatefindmatchescount", count);
  eventBus.on("updatefindcontrolstate", count);
  eventBus.on("pagesinit", () => (viewer.currentScaleValue = "page-width"));
  viewer.setDocument(doc);
  linkService.setDocument(doc);
  return {
    zoomIn: () => viewer.increaseScale(),
    zoomOut: () => viewer.decreaseScale(),
    fitWidth: () => (viewer.currentScaleValue = "page-width"),
    find: (query, options, step) =>
      eventBus.dispatch("find", {
        source: null,
        type: step ? "again" : "",
        query,
        caseSensitive: !!options.matchCase,
        entireWord: !!options.wholeWord,
        highlightAll: true,
        findPrevious: step === "previous",
        matchDiacritics: false,
      }),
    closeFind: () => eventBus.dispatch("findbarclose", { source: null }),
    onFindCount: (listener) => (counted = listener),
    destroy: () => {
      viewer.cleanup();
      void task.destroy();
    },
  };
}
