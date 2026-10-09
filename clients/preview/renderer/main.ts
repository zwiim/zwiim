// The renderer page Android previews load: it fetches the previewed file
// from `/file/<id>` (served by the native view) and renders it in place.
import { previewErrorMessage } from "../src/errors";
import { renderPdf } from "../src/pdf";
import { readSheet } from "../src/sheet";
import { renderWord } from "../src/word";
import { mountSheets, rendererParams } from "./dom";
import "./style.css";

const root = document.getElementById("root")!;
const fail = (e: unknown, name: string) => {
  const p = document.createElement("p");
  p.className = "error";
  p.textContent = previewErrorMessage(e, name) ?? `Couldn't preview ${name}.`;
  root.replaceChildren(p);
};

async function main() {
  const params = rendererParams(location.search);
  if (!params) return fail(new Error("bad link"), "this file");
  const { id, kind, name } = params;
  const url = `/file/${id}`;
  try {
    if (kind === "pdf") {
      const box = document.createElement("div");
      box.className = "pdf";
      root.replaceChildren(box);
      await renderPdf(
        box,
        { url },
        { workerSrc: "./pdfjs/pdf.worker.min.mjs", cMapUrl: "./pdfjs/cmaps/", standardFontDataUrl: "./pdfjs/standard_fonts/", wasmUrl: "./pdfjs/wasm/" },
        (e) => fail(e, name),
      );
      return;
    }
    if (kind === "image") {
      const img = document.createElement("img");
      img.src = url;
      img.alt = name;
      img.onerror = () => fail(new Error("image"), name);
      root.replaceChildren(img);
      return;
    }
    const res = await fetch(url);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (kind === "sheet") {
      root.replaceChildren();
      mountSheets(root, readSheet(bytes, name).sheets);
    } else {
      const article = document.createElement("article");
      article.innerHTML = await renderWord(bytes);
      root.replaceChildren(article);
    }
  } catch (e) {
    fail(e, name);
  }
}

void main();
