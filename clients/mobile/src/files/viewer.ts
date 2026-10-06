// A changed file as the Diff screen shows it: the whole file as it is now,
// its uncommitted changes marked (`git.diff` for the file, `files.read` for
// its text), or why it can't be shown. Pure; the screen renders it.

import { fullFile, isClientError, type DiffFile, type FileContent, type FileLine, type WorkspaceDiff } from "@zwiim/sdk";

export const TOO_LARGE = "File is too large to open";

export type Viewed =
  | { kind: "lines"; lines: FileLine[]; file: DiffFile | null; content: string | null }
  | { kind: "binary"; file: DiffFile | null }
  | { kind: "too_large"; file: DiffFile | null }
  | { kind: "gone" };

/** What `files.read` answered: the text, or that it can't be read as text. */
export interface FileRead {
  content: string | null;
  refused: "too_large" | "binary" | null;
}

/** `files.read` refuses a file too big to open, or not text; neither is an error here. */
export function readRefusal(e: unknown): FileRead["refused"] {
  if (!isClientError(e, "invalid_input")) return null;
  if (/too large/i.test(e.message)) return "too_large";
  if (/binary/i.test(e.message)) return "binary";
  return null;
}

/** `diff` is null when the file has no uncommitted changes. */
export function viewedFile(path: string, diff: WorkspaceDiff | null, read: FileRead): Viewed {
  const file = diff?.files.find((f) => f.path === path) ?? null;
  const patch = diff?.patch ?? null;
  if (file?.binary || read.refused === "binary") return { kind: "binary", file };
  if (file?.too_large || (file && patch === null) || read.refused === "too_large") return { kind: "too_large", file };
  if (file?.change === "deleted") return { kind: "lines", lines: fullFile(patch, null), file, content: null };
  if (file) return { kind: "lines", lines: fullFile(patch, read.content), file, content: read.content };
  if (read.content !== null) return { kind: "lines", lines: fullFile(null, read.content), file: null, content: read.content };
  return { kind: "gone" };
}

/** Both answers at once. A file with no changes, or none on disk, is an answer; anything else fails. */
export async function loadViewed(path: string, diff: Promise<WorkspaceDiff>, read: Promise<FileContent>): Promise<Viewed> {
  const [d, r] = await Promise.all([
    diff.catch((e: unknown) => {
      if (isClientError(e, "not_found")) return null;
      throw e;
    }),
    read.then(
      (f): FileRead => ({ content: f.content, refused: null }),
      (e: unknown): FileRead => {
        const refused = readRefusal(e);
        if (refused || isClientError(e, "not_found")) return { content: null, refused };
        throw e;
      },
    ),
  ]);
  return viewedFile(path, d, r);
}
