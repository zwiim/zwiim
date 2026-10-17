// A whole file with its changes marked, as the diff tab shows it: unchanged
// stretches folded behind bars, in one column or old beside new.

import { Fragment } from "react";
import { ChevronsUpDown } from "lucide-react";
import { fileSegments, revealStep, type FileLine } from "@zwiim/sdk";
import type { DiffLayout } from "@/lib/prefs";
import { splitRows } from "@/lib/split";
import { cn } from "@/lib/utils";

// Line tints are theme tokens (`--add-bg`/`--del-bg`), tuned for light and dark.
const TINT: Record<FileLine["kind"], string> = {
  add: "bg-add",
  del: "bg-del",
  ctx: "",
};
const SIGN: Record<FileLine["kind"], string> = { add: "+", del: "-", ctx: "" };
const SIGN_COLOR: Record<FileLine["kind"], string> = {
  add: "text-done",
  del: "text-destructive",
  ctx: "",
};

export function FileChanges({
  lines,
  layout,
  opened,
  onReveal,
}: {
  lines: readonly FileLine[];
  layout: DiffLayout;
  /** Lines revealed per fold, keyed by the fold's first line. */
  opened: Readonly<Record<number, number>>;
  onReveal(at: number, more: number): void;
}) {
  const segments = fileSegments(lines, opened);
  const columns = layout === "split" ? 6 : 4;
  return (
    <table className="w-full table-fixed border-collapse font-mono text-xs leading-5" data-testid="file-changes">
      <colgroup>
        {layout === "split" ? (
          <>
            <col className="w-11" />
            <col className="w-5" />
            <col />
            <col className="w-11" />
            <col className="w-5" />
            <col />
          </>
        ) : (
          <>
            <col className="w-11" />
            <col className="w-11" />
            <col className="w-5" />
            <col />
          </>
        )}
      </colgroup>
      <tbody>
        {segments.map((s) =>
          s.kind === "fold" ? (
            <tr key={`fold-${s.at}`}>
              <td colSpan={columns} className="p-0">
                <button
                  className="bg-muted text-muted-foreground hover:bg-accent hover:text-foreground flex w-full items-center gap-2 px-3 py-1 text-left transition-colors"
                  onClick={() => onReveal(s.at, revealStep(s.count))}
                  data-testid="fold"
                >
                  <ChevronsUpDown className="size-3.5" />
                  <span className="tabular-nums">
                    {s.count} unmodified {s.count === 1 ? "line" : "lines"}
                  </span>
                </button>
              </td>
            </tr>
          ) : layout === "split" ? (
            <Fragment key={`lines-${s.from}`}>
              {splitRows(lines.slice(s.from, s.to)).map((r, i) => (
                <tr key={i} className="align-top">
                  <SplitHalf line={r.left} side="old" />
                  <SplitHalf line={r.right} side="new" />
                </tr>
              ))}
            </Fragment>
          ) : (
            <Fragment key={`lines-${s.from}`}>
              {lines.slice(s.from, s.to).map((l, i) => (
                <tr key={s.from + i} className={cn("align-top", TINT[l.kind])} data-kind={l.kind}>
                  <Gutter n={l.old} kind={l.kind} />
                  <Gutter n={l.new} kind={l.kind} />
                  <td className={cn("text-center select-none", SIGN_COLOR[l.kind])}>{SIGN[l.kind]}</td>
                  <Text text={l.text} />
                </tr>
              ))}
            </Fragment>
          ),
        )}
      </tbody>
    </table>
  );
}

function Gutter({ n, kind }: { n: number | null; kind: FileLine["kind"] }) {
  return (
    <td className={cn("text-muted-foreground pr-2 text-right tabular-nums select-none", TINT[kind])}>
      {n ?? ""}
    </td>
  );
}

function Text({ text }: { text: string }) {
  return <td className="pr-4 break-all whitespace-pre-wrap">{text || " "}</td>;
}

/** One side of a split row; a side with no line is a faint blank. */
function SplitHalf({ line, side }: { line: FileLine | null; side: "old" | "new" }) {
  const edge = side === "new" && "border-l";
  if (!line) return <td colSpan={3} className={cn("bg-muted", edge)} />;
  return (
    <>
      <td className={cn("text-muted-foreground pr-2 text-right tabular-nums select-none", TINT[line.kind], edge)}>
        {side === "old" ? line.old : line.new}
      </td>
      <td className={cn("text-center select-none", TINT[line.kind], SIGN_COLOR[line.kind])}>{SIGN[line.kind]}</td>
      <td className={cn("pr-4 break-all whitespace-pre-wrap", TINT[line.kind])}>{line.text || " "}</td>
    </>
  );
}
