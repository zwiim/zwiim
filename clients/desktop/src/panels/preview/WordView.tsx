import type { MouseEvent } from "react";
import { isOpenableLink, openLink } from "@/lib/bridge";

// The Markdown component's document typography, applied to Word's HTML.
const DOCUMENT = [
  "text-sm leading-6 break-words",
  "[&_p]:my-2.5 [&_ul]:my-2 [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:my-2 [&_ol]:list-decimal [&_ol]:pl-5 [&_li]:my-0.5",
  "[&_h1]:mt-5 [&_h1]:mb-2 [&_h1]:text-lg [&_h1]:font-semibold [&_h1]:tracking-tight",
  "[&_h2]:mt-5 [&_h2]:mb-2 [&_h2]:text-base [&_h2]:font-semibold [&_h2]:tracking-tight",
  "[&_h3]:mt-4 [&_h3]:mb-1.5 [&_h3]:font-semibold",
  "[&_table]:my-2 [&_table]:border-collapse [&_table]:text-xs [&_td]:border [&_td]:px-2.5 [&_td]:py-1.5 [&_th]:border [&_th]:px-2.5 [&_th]:py-1.5",
  "[&_a]:underline [&_a]:underline-offset-2 [&_img]:max-w-full",
].join(" ");

/** Sanitized Word HTML; links leave for the browser, the app never navigates. */
export function WordView({ html }: { html: string }) {
  function onClick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute("href") ?? "";
    if (isOpenableLink(href)) void openLink(href);
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto p-6">
      <article className={`max-w-3xl ${DOCUMENT}`} onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}
