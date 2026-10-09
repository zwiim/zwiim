import { SHEET_CAP_NOTE, type Sheet } from "../src/sheet";

export function rendererParams(search: string): { id: string; kind: string; name: string } | null {
  const q = new URLSearchParams(search);
  const id = q.get("id") ?? "";
  const kind = q.get("kind") ?? "";
  if (!/^[0-9a-f]{32}$/.test(id) || !["pdf", "sheet", "word", "image"].includes(kind)) return null;
  return { id, kind, name: q.get("name") ?? "" };
}

export function mountSheets(container: HTMLElement, sheets: Sheet[]): void {
  const tabs = document.createElement("div");
  tabs.setAttribute("role", "tablist");
  tabs.className = "tabs";
  const body = document.createElement("div");
  body.className = "sheet";
  const show = (at: number) => {
    const sheet = sheets[at]!;
    tabs.querySelectorAll("[role=tab]").forEach((t, i) => t.setAttribute("aria-selected", String(i === at)));
    body.replaceChildren();
    if (sheet.truncated) {
      const note = document.createElement("p");
      note.className = "note";
      note.textContent = SHEET_CAP_NOTE;
      body.append(note);
    }
    const table = document.createElement("table");
    for (const row of sheet.rows) {
      const tr = table.insertRow();
      for (const cell of row) tr.insertCell().textContent = cell;
    }
    body.append(table);
  };
  if (sheets.length > 1) {
    sheets.forEach((s, i) => {
      const b = document.createElement("button");
      b.setAttribute("role", "tab");
      b.textContent = s.name;
      b.onclick = () => show(i);
      tabs.append(b);
    });
    container.append(tabs);
  }
  container.append(body);
  show(0);
}
