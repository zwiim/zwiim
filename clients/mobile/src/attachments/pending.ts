// The files chosen for the next message. They upload into the workspace the
// message goes to when it is sent; once uploaded there they are not uploaded
// again, and a file that failed is tried again on the next send.

import { checkAttachments, isClientError, type PromptBlock } from "@zwiim/sdk";

/** A file picked on this phone, read from `uri` when it uploads. */
export interface PickedFile {
  name: string;
  size: number;
  mime_type: string;
  uri: string;
}

export interface PendingFile extends PickedFile {
  id: string;
  status: "ready" | "uploading" | "uploaded" | "failed";
  /** Set once uploaded, with the workspace it went to. */
  attachment_id?: string;
  workspace_id?: string;
  /** Why the last upload failed. */
  error?: string;
}

export interface PendingState {
  files: PendingFile[];
  /** Refusals from the last add: too large, too many. */
  errors: string[];
}

export type PendingAction =
  | { type: "add"; files: (PickedFile & { id: string })[] }
  | { type: "remove"; id: string }
  | { type: "uploading"; id: string }
  | { type: "uploaded"; id: string; attachment_id: string; workspace_id: string }
  | { type: "failed"; id: string; error: string }
  /** The Host no longer has them: upload them again on the next send. */
  | { type: "forget_uploads" }
  | { type: "clear" };

export const EMPTY_PENDING: PendingState = { files: [], errors: [] };

/** What to say when a send stopped because a file did not upload. */
export const UPLOAD_FAILED_MESSAGE = "A file did not upload. Send again to retry it, or remove it.";

function update(state: PendingState, id: string, change: Partial<PendingFile>): PendingState {
  return { ...state, files: state.files.map((f) => (f.id === id ? { ...f, ...change } : f)) };
}

export function pendingReducer(state: PendingState, action: PendingAction): PendingState {
  switch (action.type) {
    case "add": {
      const { accepted, errors } = checkAttachments(state.files.length, action.files);
      return { files: [...state.files, ...accepted.map((f) => ({ ...f, status: "ready" as const }))], errors };
    }
    case "remove":
      return { files: state.files.filter((f) => f.id !== action.id), errors: [] };
    case "uploading":
      return update(state, action.id, { status: "uploading", error: undefined });
    case "uploaded":
      return update(state, action.id, {
        status: "uploaded",
        attachment_id: action.attachment_id,
        workspace_id: action.workspace_id,
      });
    case "failed":
      return update(state, action.id, { status: "failed", error: action.error });
    case "forget_uploads":
      return {
        ...state,
        files: state.files.map((f) =>
          f.status === "uploaded" ? { ...f, status: "ready", attachment_id: undefined, workspace_id: undefined } : f,
        ),
      };
    case "clear":
      return EMPTY_PENDING;
  }
}

/** Whether `file` still needs uploading to `workspace_id`. */
export function needsUpload(file: PendingFile, workspace_id: string): boolean {
  return file.status !== "uploaded" || file.workspace_id !== workspace_id;
}

/**
 * Upload what is not yet in `workspace_id`, one at a time (each may be 10 MB
 * through the relay), reporting each step. The attachment blocks in order, or
 * null when any failed.
 */
export async function uploadPending(
  files: readonly PendingFile[],
  workspace_id: string,
  put: (file: PendingFile) => Promise<string>,
  dispatch: (action: PendingAction) => void,
  describe: (e: unknown) => string,
): Promise<PromptBlock[] | null> {
  const ids: (string | null)[] = [];
  for (const f of files) {
    if (!needsUpload(f, workspace_id)) {
      ids.push(f.attachment_id!);
      continue;
    }
    dispatch({ type: "uploading", id: f.id });
    try {
      const attachment_id = await put(f);
      dispatch({ type: "uploaded", id: f.id, attachment_id, workspace_id });
      ids.push(attachment_id);
    } catch (e) {
      dispatch({ type: "failed", id: f.id, error: describe(e) });
      ids.push(null);
    }
  }
  if (ids.includes(null)) return null;
  return ids.map((attachment_id) => ({ type: "attachment", attachment_id: attachment_id! }));
}

/** A picked file's name: the one the picker gave, else the last part of its URI. */
export function fileNameOf(uri: string, name?: string | null): string {
  const given = name?.trim();
  if (given) return given;
  const last = uri.split(/[?#]/)[0]!.split("/").filter(Boolean).pop();
  try {
    return last ? decodeURIComponent(last) : "File";
  } catch {
    return last ?? "File";
  }
}

/** Why one file did not upload, in a chip's words. */
export function uploadError(e: unknown, hostName: string): string {
  if (isClientError(e, "attachment_too_large")) return "Larger than 10 MB.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}.`;
  if (isClientError(e, "not_found") || isClientError(e, "workspace_removed")) return "That worktree is gone.";
  return "Couldn't upload it.";
}
