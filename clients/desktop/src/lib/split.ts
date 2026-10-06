// A changed file side by side: old on the left, new on the right.

import type { FileLine } from "@zwiim/sdk";

export interface SplitRow {
  left: FileLine | null;
  right: FileLine | null;
}

/**
 * Rows for the split view. An unchanged line shows on both sides; a run of
 * removed lines pairs row by row with the added run after it, and the longer
 * run's extra lines face a blank.
 */
export function splitRows(lines: readonly FileLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (line.kind === "ctx") {
      rows.push({ left: line, right: line });
      i++;
      continue;
    }
    const dels: FileLine[] = [];
    const adds: FileLine[] = [];
    while (i < lines.length && lines[i]!.kind === "del") dels.push(lines[i++]!);
    while (i < lines.length && lines[i]!.kind === "add") adds.push(lines[i++]!);
    for (let j = 0; j < Math.max(dels.length, adds.length); j++) {
      rows.push({ left: dels[j] ?? null, right: adds[j] ?? null });
    }
  }
  return rows;
}
