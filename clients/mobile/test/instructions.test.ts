import { describe, expect, it } from "vitest";
import {
  CLIENT_CAPABILITIES,
  ZwiimClientError,
  type Compatibility,
  type UserInstructions,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import {
  instructionScopes,
  instructionsOffered,
  loadInstructions,
  saveInstructions,
  type InstructionsHost,
} from "../src/host/instructions";

const workspace = (extra: Partial<WorkspaceInfo>): WorkspaceInfo => ({
  workspace_id: "root",
  name: "repo",
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...extra,
});
const root = workspace({ alias: "Zwiim" });
const worktree = workspace({ workspace_id: "a", name: "repo-a", kind: "worktree", parent_workspace_id: "root", branch: "zwiim/a" });

const saved: UserInstructions = {
  host: { general: "Be brief.", resolve_conflicts: null },
  workspaces: { root: { general: null, resolve_conflicts: "Prefer main's schema." } },
};

function fakeHost(answer: (op: string, params: unknown) => unknown) {
  const calls: { op: string; params: unknown }[] = [];
  const host: InstructionsHost = {
    request: (async (op: string, params: unknown) => {
      calls.push({ op, params });
      return answer(op, params);
    }) as InstructionsHost["request"],
  };
  return { host, calls };
}

describe("instructions on the phone", () => {
  it("are offered only by a Host that serves them", () => {
    const compat = (capabilities: string[]) => ({ status: "ok", capabilities }) as unknown as Compatibility;
    expect(instructionsOffered(compat([...CLIENT_CAPABILITIES]))).toBe(true);
    expect(instructionsOffered(compat(CLIENT_CAPABILITIES.filter((c) => c !== "host.instructions")))).toBe(false);
    expect(instructionsOffered(null)).toBe(false);
  });

  it("list every workspace, then the current worktree's repository", () => {
    expect(instructionScopes(saved, [root, worktree], worktree)).toEqual([
      { id: "host", title: "Every workspace", note: null, texts: saved.host },
      { id: "root", title: "Zwiim", note: "Its worktrees use these too.", texts: saved.workspaces.root },
    ]);
    // The root itself names the same repository; nothing set reads as empty.
    expect(instructionScopes({ ...saved, workspaces: {} }, [root, worktree], root)[1]).toEqual({
      id: "root",
      title: "Zwiim",
      note: "Its worktrees use these too.",
      texts: { general: null, resolve_conflicts: null },
    });
    expect(instructionScopes(saved, [root, worktree], null).map((s) => s.id)).toEqual(["host"]);
    expect(instructionScopes(saved, [{ ...root, removed: true }, worktree], worktree).map((s) => s.id)).toEqual(["host"]);
  });

  it("save both texts of a scope, and say why the Host refused", async () => {
    const ok = fakeHost(() => saved);
    expect(await saveInstructions(ok.host, "host", { general: "Be brief.", resolve_conflicts: "" })).toEqual({ value: saved });
    expect(await saveInstructions(ok.host, "root", { general: "Run tests.", resolve_conflicts: "Keep ours." })).toEqual({
      value: saved,
    });
    expect(ok.calls).toEqual([
      { op: "host.instructions.set", params: { scope: "host", general: "Be brief.", resolve_conflicts: "" } },
      {
        op: "host.instructions.set",
        params: { scope: "workspace", workspace_id: "root", general: "Run tests.", resolve_conflicts: "Keep ours." },
      },
    ]);
    const refuse = (e: Error) =>
      fakeHost(() => {
        throw e;
      }).host;
    const texts = { general: "x", resolve_conflicts: "" };
    expect(await saveInstructions(refuse(new ZwiimClientError("too_large", "big", 413)), "host", texts)).toEqual({
      error: "Each text can be at most 8 KiB.",
    });
    expect(await saveInstructions(refuse(new ZwiimClientError("not_found", "gone", 404)), "root", texts)).toEqual({
      error: "This workspace was removed.",
    });
    expect(await saveInstructions(refuse(new ZwiimClientError("network", "Host unreachable")), "host", texts)).toEqual({
      error: "Host unreachable",
    });
  });

  it("loads what the Host holds, or says why it couldn't", async () => {
    const value: UserInstructions = { host: { general: "Be brief.", resolve_conflicts: null }, workspaces: {} };
    const ok = fakeHost(() => value);
    expect(await loadInstructions(ok.host)).toEqual({ value });
    expect(ok.calls).toEqual([{ op: "host.instructions", params: {} }]);
    const down = fakeHost(() => {
      throw new ZwiimClientError("network", "Host unreachable");
    });
    expect(await loadInstructions(down.host)).toEqual({ error: "Host unreachable" });
  });
});
