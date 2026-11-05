import { useCallback, useReducer, useRef } from "react";
import { randomUUID } from "expo-crypto";
import { hasCapability, uploadAttachment, type PromptBlock } from "@zwiim/sdk";
import { useCurrentHost } from "../app/hooks";
import { currentHost } from "../services";
import { readBase64 } from "./native";
import { pendingReducer, uploadError, uploadPending, type PendingFile, type PickedFile } from "./pending";

/**
 * The files chosen for the next message, and their upload into the workspace
 * the message goes to. `initial` are files already chosen, carried from another
 * composer. `enabled` is false against a Host without attachments.
 */
export function usePendingAttachments(hostName: string, initial: PendingFile[] = []) {
  const { view } = useCurrentHost();
  const enabled = hasCapability(view?.compat ?? null, "attachments");
  const [state, dispatch] = useReducer(pendingReducer, { files: initial, errors: [] });
  const current = useRef(state);
  current.current = state;

  const add = useCallback((files: PickedFile[]) => {
    if (files.length > 0) dispatch({ type: "add", files: files.map((f) => ({ ...f, id: randomUUID() })) });
  }, []);

  /** The attachment blocks in order, or null when a file did not upload; its chip says why. */
  const upload = useCallback(
    (workspace_id: string): Promise<PromptBlock[] | null> =>
      uploadPending(
        current.current.files,
        workspace_id,
        async (f) => (await uploadAttachment(currentHost, workspace_id, f.name, await readBase64(f.uri))).attachment_id,
        dispatch,
        (e) => uploadError(e, hostName),
      ),
    [hostName],
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
