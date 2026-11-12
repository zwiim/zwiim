import { useEffect, useRef, useState, type RefObject } from "react";
import { findMatches, type FindOptions } from "@zwiim/preview";

/** Text in different ones of these never joins into one match: a match stays in its paragraph or cell. */
const BLOCK = "p,li,td,th,h1,h2,h3,h4,h5,h6,pre,blockquote,dt,dd,caption,figcaption,div,section,article";

/** Ranges of `query` in the text under `scope`, each inside one block. */
export function domMatches(scope: HTMLElement, query: string, options: FindOptions): Range[] {
  if (!query) return [];
  const nodes: { node: Text; start: number }[] = [];
  let text = "";
  let block: Element | null = null;
  // Controls inside the preview (`data-find-skip`), like a workbook's sheet tabs, are not its text.
  const walk = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement?.closest("[data-find-skip]") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const node = n as Text;
    const at = node.parentElement?.closest(BLOCK) ?? null;
    // A query never holds a newline, so one between blocks keeps matches apart.
    if (nodes.length && at !== block) text += "\n";
    block = at;
    nodes.push({ node, start: text.length });
    text += node.data;
  }
  let i = 0;
  const locate = (offset: number, end: boolean) => {
    // Matches run forward, so the search for their nodes does too.
    while (i + 1 < nodes.length && (end ? nodes[i + 1]!.start < offset : nodes[i + 1]!.start <= offset)) i++;
    const { node, start } = nodes[i]!;
    return [node, offset - start] as const;
  };
  return findMatches(text, query, options).map((m) => {
    const range = document.createRange();
    range.setStart(...locate(m.start, false));
    range.setEnd(...locate(m.end, true));
    return range;
  });
}

/**
 * Moves this preview's ranges in the named highlight from `from` to `to`.
 * Every tab's find shares the two highlights, so each touches only its own.
 */
function paint(name: string, from: Range[], to: Range[]) {
  if (typeof CSS === "undefined" || !("highlights" in CSS) || typeof Highlight === "undefined") return;
  let h = CSS.highlights.get(name);
  if (!h) CSS.highlights.set(name, (h = new Highlight()));
  for (const r of from) h.delete(r);
  for (const r of to) h.add(r);
}

/**
 * Find in a rendered preview's DOM under `scope`, re-run when that DOM
 * changes. Matches are painted with the CSS Custom Highlight API where the
 * webview has it, so React's DOM is never touched; the current one is
 * scrolled to either way. Returns the match count.
 */
export function useDomFind(
  scope: RefObject<HTMLElement | null>,
  enabled: boolean,
  query: string,
  options: FindOptions,
  current: number,
): number {
  const [ranges, setRanges] = useState<Range[]>([]);
  const [changes, setChanges] = useState(0);
  const painted = useRef<{ all: Range[]; current: Range[] }>({ all: [], current: [] });

  useEffect(() => {
    const el = scope.current;
    if (!enabled || !el) return;
    const seen = new MutationObserver(() => setChanges((n) => n + 1));
    seen.observe(el, { childList: true, subtree: true, characterData: true });
    return () => seen.disconnect();
  }, [scope, enabled]);

  useEffect(() => {
    const el = scope.current;
    setRanges(enabled && el ? domMatches(el, query, options) : []);
  }, [scope, enabled, query, options, changes]);

  useEffect(() => {
    const range = ranges[Math.min(current, ranges.length - 1)];
    const next = { all: ranges.filter((r) => r !== range), current: range ? [range] : [] };
    paint("find-match", painted.current.all, next.all);
    paint("find-current", painted.current.current, next.current);
    painted.current = next;
    range?.startContainer.parentElement?.scrollIntoView?.({ block: "center", inline: "nearest" });
  }, [ranges, current]);

  useEffect(
    () => () => {
      paint("find-match", painted.current.all, []);
      paint("find-current", painted.current.current, []);
    },
    [],
  );
  return ranges.length;
}
