// The user's instructions for their agents, from the phone: Host-wide, and
// for the current worktree's repository, which its worktrees share.

import {
  hasCapability,
  instructionsErrorText,
  type Compatibility,
  type InstructionTexts,
  type OpName,
  type OpParams,
  type OpResult,
  type UserInstructions,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { errorInfo } from "./connection";
import { rootIdOf, workspaceLabel } from "./workspace";

/** Whether the Host keeps instructions; an older one has none to edit. */
export function instructionsOffered(compat: Compatibility | null): boolean {
  return hasCapability(compat, "host.instructions");
}

export interface InstructionsHost {
  request<O extends OpName>(op: O, params: OpParams<O>): Promise<OpResult<O>>;
}

/** One row of the Instructions group: `id` is "host" or a root's `workspace_id`. */
export interface InstructionScope {
  id: string;
  title: string;
  note: string | null;
  texts: InstructionTexts;
}

/** Drafts as typed: both texts of a scope, empty for none. */
export type InstructionDrafts = { [K in keyof InstructionTexts]: string };

/** Every workspace, then the current worktree's repository while the Host still lists it. */
export function instructionScopes(
  value: UserInstructions,
  workspaces: readonly WorkspaceInfo[] | null,
  current: WorkspaceInfo | null,
): InstructionScope[] {
  const scopes: InstructionScope[] = [{ id: "host", title: "Every workspace", note: null, texts: value.host }];
  const rootId = current ? rootIdOf(current) : null;
  const root = (workspaces ?? []).find((w) => w.workspace_id === rootId && !w.removed);
  if (root) {
    scopes.push({
      id: root.workspace_id,
      title: workspaceLabel(root),
      note: "Its worktrees use these too.",
      texts: value.workspaces[root.workspace_id] ?? { general: null, resolve_conflicts: null },
    });
  }
  return scopes;
}

/** What the Host holds, or why it couldn't be read. */
export async function loadInstructions(host: InstructionsHost): Promise<{ value: UserInstructions } | { error: string }> {
  try {
    return { value: await host.request("host.instructions", {}) };
  } catch (e) {
    return { error: errorInfo(e).message };
  }
}

/** Save both texts of scope `id`; what the Host then holds, or why it refused. */
export async function saveInstructions(
  host: InstructionsHost,
  id: string,
  drafts: InstructionDrafts,
): Promise<{ value: UserInstructions } | { error: string }> {
  const params = id === "host" ? { scope: "host" as const, ...drafts } : { scope: "workspace" as const, workspace_id: id, ...drafts };
  try {
    return { value: await host.request("host.instructions.set", params) };
  } catch (e) {
    return { error: instructionsErrorText(e) ?? errorInfo(e).message };
  }
}
