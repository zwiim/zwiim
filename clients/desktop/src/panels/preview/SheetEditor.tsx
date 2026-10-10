import { memo, useCallback, useRef, useState, type KeyboardEvent } from "react";
import { Plus, Trash2 } from "lucide-react";
import { addRow, deleteRow, delimitedWidth, setCell, type DelimitedRow, type DelimitedTable } from "@zwiim/preview";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type Active = { r: number; c: number; row: DelimitedRow };

/**
 * A CSV/TSV as an editable grid. A cell opens on click and edits the table as
 * you type; Enter or Tab closes it, Escape puts the cell back. Changed rows are
 * tinted — only they are rewritten on save.
 */
export function SheetEditor({ table, onChange }: { table: DelimitedTable; onChange: (table: DelimitedTable) => void }) {
  const [active, setActive] = useState<Active | null>(null);
  const width = Math.max(1, delimitedWidth(table));
  // Row callbacks stay stable so typing re-renders only the open row.
  const latest = useRef({ table, onChange, width, active });
  latest.current = { table, onChange, width, active };

  const open = useCallback((r: number, c: number, from = latest.current.table) => {
    const row = from.rows[r];
    if (row) setActive({ r, c, row });
  }, []);
  const remove = useCallback((r: number) => {
    setActive(null);
    latest.current.onChange(deleteRow(latest.current.table, r));
  }, []);
  const edit = useCallback((r: number, c: number, value: string) => {
    latest.current.onChange(setCell(latest.current.table, r, c, value));
  }, []);
  // Closing a cell typed back to what it was keeps its row's source text.
  const close = useCallback(
    (restore: boolean, move?: number) => {
      const { table, onChange, width, active } = latest.current;
      if (!active) return;
      let next = table;
      const row = table.rows[active.r];
      const same = row && (row.cells[active.c] ?? "") === (active.row.cells[active.c] ?? "");
      if (row && row !== active.row && (restore || (same && row.cells.length === active.row.cells.length))) {
        next = { ...table, rows: table.rows.map((x, i) => (i === active.r ? active.row : x)) };
        onChange(next);
      }
      const at = move === undefined ? -1 : active.r * width + active.c + move;
      if (at >= 0 && at < next.rows.length * width) open(Math.floor(at / width), at % width, next);
      else setActive(null);
    },
    [open],
  );

  function append() {
    const next = addRow(table);
    onChange(next);
    setActive({ r: table.rows.length, c: 0, row: next.rows.at(-1)! });
  }

  return (
    <div className="min-h-0 flex-1 overflow-auto p-4" data-testid="sheet-editor">
      <table className="border-collapse font-mono text-xs">
        <tbody>
          {table.rows.map((row, r) => (
            <EditorRow
              key={r}
              row={row}
              r={r}
              width={width}
              activeCol={active?.r === r ? active.c : -1}
              onOpen={open}
              onEdit={edit}
              onClose={close}
              onDelete={remove}
            />
          ))}
        </tbody>
      </table>
      <Button className="mt-3" size="sm" variant="outline" onClick={append}>
        <Plus /> Add row
      </Button>
    </div>
  );
}

const EditorRow = memo(function EditorRow({
  row,
  r,
  width,
  activeCol,
  onOpen,
  onEdit,
  onClose,
  onDelete,
}: {
  row: DelimitedRow;
  r: number;
  width: number;
  activeCol: number;
  onOpen: (r: number, c: number) => void;
  onEdit: (r: number, c: number, value: string) => void;
  onClose: (restore: boolean, move?: number) => void;
  onDelete: (r: number) => void;
}) {
  function keyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      onClose(false);
    } else if (e.key === "Tab") {
      e.preventDefault();
      onClose(false, e.shiftKey ? -1 : 1);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose(true);
    }
  }
  return (
    <tr className={cn(row.raw === null && "bg-primary/5")}>
      <td className="pr-1 align-top">
        <Button size="icon-sm" variant="ghost" title={`Delete row ${r + 1}`} onClick={() => onDelete(r)}>
          <Trash2 />
        </Button>
      </td>
      {Array.from({ length: width }, (_, c) => {
        const value = row.cells[c] ?? "";
        return c === activeCol ? (
          <td key={c} className="border p-0 align-top">
            <textarea
              aria-label="Cell"
              autoFocus
              rows={Math.max(1, value.split("\n").length)}
              className="block min-w-24 resize-none bg-transparent px-2 py-1 outline-none"
              value={value}
              onChange={(e) => onEdit(r, c, e.target.value)}
              onKeyDown={keyDown}
              // A cell removed by Enter or Tab has already closed.
              onBlur={(e) => e.currentTarget.isConnected && onClose(false)}
            />
          </td>
        ) : (
          <td key={c} className="hover:bg-muted min-w-12 cursor-text border px-2 py-1 align-top whitespace-pre" onClick={() => onOpen(r, c)}>
            {value}
          </td>
        );
      })}
    </tr>
  );
});
