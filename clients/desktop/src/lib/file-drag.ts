// A file dragged out of the Files panel onto a browser page (browser design
// 7.6). WebKit shows a drag's data only at the drop, so the file is held here
// while the drag lasts; the drag itself carries just the path as text.

import type { DragEvent } from "react";

/** A workspace file, by its path relative to the workspace. */
export interface DraggedFile {
  workspaceId: string;
  path: string;
}

let dragged: DraggedFile | null = null;

/** Props that make a Files panel row a drag source for `path`. */
export function fileDragProps(workspaceId: string, path: string) {
  return {
    draggable: true,
    onDragStart(e: DragEvent<HTMLElement>) {
      dragged = { workspaceId, path };
      e.dataTransfer.setData("text/plain", path);
      e.dataTransfer.effectAllowed = "copy";
    },
    onDragEnd() {
      dragged = null;
    },
  };
}

/** The workspace file being dragged now, if any. */
export function draggedFile(): DraggedFile | null {
  return dragged;
}
