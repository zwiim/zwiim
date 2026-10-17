import { useState } from "react";
import { SHEET_CAP_NOTE, type Sheet } from "@zwiim/preview/sheet";
import { cn } from "@/lib/utils";

/** A workbook's sheets as tabs, each a plain table of cell text. */
export function SheetView({ sheets }: { sheets: Sheet[] }) {
  const [at, setAt] = useState(0);
  const sheet = sheets[Math.min(at, sheets.length - 1)]!;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {sheets.length > 1 && (
        <div role="tablist" className="flex shrink-0 gap-1 overflow-x-auto border-b px-4 py-1.5">
          {sheets.map((s, i) => (
            <button
              key={s.name}
              role="tab"
              aria-selected={i === at}
              className={cn(
                "rounded-sm px-2 py-1 text-xs transition-colors",
                i === at ? "bg-accent text-foreground" : "text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
              onClick={() => setAt(i)}
            >
              {s.name}
            </button>
          ))}
        </div>
      )}
      {sheet.truncated && <p className="text-muted-foreground shrink-0 px-4 pt-2 text-xs">{SHEET_CAP_NOTE}</p>}
      <div key={at} className="animate-fade-in min-h-0 flex-1 overflow-auto p-4">
        <table className="border-collapse font-mono text-xs" data-testid="sheet-table">
          <tbody>
            {sheet.rows.map((row, r) => (
              <tr key={r}>
                {row.map((cell, c) => (
                  <td key={c} className="border px-2 py-1 whitespace-pre">
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
