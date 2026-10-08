import { checkAttachments } from "@zwiim/sdk";

/**
 * A file chosen for the next message. It uploads when the message is sent;
 * once uploaded to a workspace it is not uploaded there again.
 */
export interface PendingFile {
  id: string;
  name: string;
  size: number;
  mime_type: string;
  file: Blob;
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
  | { type: "add"; files: Pick<PendingFile, "id" | "name" | "size" | "mime_type" | "file">[] }
  | { type: "remove"; id: string }
  | { type: "uploading"; id: string }
  | { type: "uploaded"; id: string; attachment_id: string; workspace_id: string }
  | { type: "failed"; id: string; error: string }
  /** The Host no longer has them: upload them again on the next send. */
  | { type: "forget_uploads" }
  | { type: "clear" };

export const EMPTY_PENDING: PendingState = { files: [], errors: [] };

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
