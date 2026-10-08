import { useCallback, useReducer, useRef } from "react";
import { hasCapability, uploadAttachment, type PromptBlock } from "@zwiim/sdk";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { EMPTY_PENDING, needsUpload, pendingReducer } from "./pending";

/** What to say when a send stopped because a file did not upload. */
export const UPLOAD_FAILED_MESSAGE = "A file did not upload. Send again to retry it, or remove it.";

/** Standard base64 of a file's bytes. */
export function readBase64(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      resolve(url.slice(url.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the file."));
    reader.readAsDataURL(file);
  });
}

/**
 * The files chosen for the next message, and their upload into the workspace
 * the message goes to. `enabled` is false against a Host without attachments.
 */
export function usePendingAttachments() {
  const controller = useController();
  const enabled = useHost((s) => hasCapability(s.connection.compat, "attachments"));
  const [state, dispatch] = useReducer(pendingReducer, EMPTY_PENDING);
  const current = useRef(state);
  current.current = state;

  const add = useCallback((files: Iterable<File>) => {
    const list = [...files];
    if (list.length === 0) return;
    dispatch({
      type: "add",
      files: list.map((f) => ({
        id: crypto.randomUUID(),
        name: f.name || "Pasted file",
        size: f.size,
        mime_type: f.type || "application/octet-stream",
        file: f,
      })),
    });
  }, []);

  /**
   * Upload what is not yet in `workspace_id`, one at a time (each may be
   * 10 MB through a relay). The attachment blocks in order, or null when any
   * failed; those chips say so and the next send tries them again.
   */
  const upload = useCallback(
    async (workspace_id: string): Promise<PromptBlock[] | null> => {
      const ids: (string | null)[] = [];
      for (const f of current.current.files) {
        if (!needsUpload(f, workspace_id)) {
          ids.push(f.attachment_id!);
          continue;
        }
        dispatch({ type: "uploading", id: f.id });
        try {
          const up = await uploadAttachment(controller.client, workspace_id, f.name, await readBase64(f.file));
          dispatch({ type: "uploaded", id: f.id, attachment_id: up.attachment_id, workspace_id });
          ids.push(up.attachment_id);
        } catch (e) {
          dispatch({ type: "failed", id: f.id, error: errorMessage(e) });
          ids.push(null);
        }
      }
      if (ids.includes(null)) return null;
      return ids.map((attachment_id) => ({ type: "attachment", attachment_id: attachment_id! }));
    },
    [controller],
  );

  return {
    enabled,
    files: state.files,
    errors: state.errors,
    add,
    upload,
    remove: useCallback((id: string) => dispatch({ type: "remove", id }), []),
    /** After `attachment_not_found`: upload them all again next time. */
    forgetUploads: useCallback(() => dispatch({ type: "forget_uploads" }), []),
    clear: useCallback(() => dispatch({ type: "clear" }), []),
  };
}

