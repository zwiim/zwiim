import { describe, expect, it } from "vitest";
import { turnGroups } from "../src/changes/turns";
import { CLIENT_CAPABILITIES, evaluateCompatibility } from "../src/client/compat";
import { ZwiimClientError, type ClientErrorCode } from "../src/client/errors";
import { FORK_LABELS, forkRefusalText, forkTurnOf, offersFork } from "../src/client/fork";
import { initialSessionView, waitingTranscript, type SessionView } from "../src/state/session";
import type { ForkTarget, HistoryTurn } from "../src/types/protocol";
import { hello } from "./fakes";

const turn = (turn_id: string, role: string, fork_targets?: ForkTarget[]): HistoryTurn => ({
  turn_id,
  role,
  blocks: [],
  timestamp: "2026-01-01T00:00:00Z",
  completed_at: null,
  duration_ms: null,
  model: null,
  usage: null,
  ...(fork_targets ? { fork_targets } : {}),
});
const refusal = (code: string) => new ZwiimClientError(code as ClientErrorCode, "no", 409);

describe("forkTurnOf", () => {
  it("finds the turn the Host offered, even with a system turn after it", () => {
    const history = [
      turn("u", "user"),
      turn("a", "assistant", ["current", "new_worktree"]),
      turn("s", "system"),
    ];
    const [group] = turnGroups(history);
    expect(history[group!.end]!.turn_id).toBe("s");
    expect(forkTurnOf(history, group!)?.turn_id).toBe("a");
  });

  it("keeps each group to its own turns", () => {
    const history = [turn("u1", "user"), turn("a1", "assistant", ["current"]), turn("u2", "user"), turn("a2", "assistant")];
    const groups = turnGroups(history);
    expect(forkTurnOf(history, groups[0]!)?.turn_id).toBe("a1");
    expect(forkTurnOf(history, groups[1]!)).toBeNull();
  });

  it("ignores an empty offer", () => {
    const history = [turn("u", "user"), turn("a", "assistant", [])];
    expect(forkTurnOf(history, turnGroups(history)[0]!)).toBeNull();
  });
});

describe("offersFork", () => {
  it("needs a Host with session.fork", () => {
    expect(offersFork(evaluateCompatibility(hello({ capabilities: [...CLIENT_CAPABILITIES] })))).toBe(true);
    expect(offersFork(evaluateCompatibility(hello({ capabilities: [] })))).toBe(false);
    expect(offersFork(null)).toBe(false);
  });
});

describe("forkRefusalText", () => {
  it("says why for each refusal", () => {
    expect(forkRefusalText(refusal("fork_unavailable"))).toBe("This turn can't be forked.");
    expect(forkRefusalText(refusal("turn_in_progress"))).toBe("The agent is still working. Wait for it to finish, or stop it, then try again.");
    expect(forkRefusalText(refusal("checkpoint_unavailable"))).toBe("The files at this point are no longer available.");
    expect(forkRefusalText(refusal("root_workspace"))).toBe("This session ran in the repository folder, so it can only fork to a new worktree.");
    expect(forkRefusalText(refusal("not_found"))).toBe("This session is gone.");
  });

  it("leaves other failures to the app", () => {
    expect(forkRefusalText(refusal("internal"))).toBeNull();
    expect(forkRefusalText(new Error("offline"))).toBeNull();
  });
});

describe("the composer's waiting transcript", () => {
  const pendingTranscript = {
    type: "attachment" as const,
    attachment_id: "a1",
    name: "Transcript of Old.md",
    mime_type: "text/markdown",
    path: ".zwiim/attachments/a1/Transcript of Old.md",
  };
  const view: SessionView = { ...initialSessionView("s1"), pendingTranscript };

  it("names the two ways to fork", () => {
    expect(FORK_LABELS).toEqual({ current: "Fork to new tab", new_worktree: "Fork to new worktree" });
  });

  it("shows until a turn takes the message that carries it", () => {
    expect(waitingTranscript(view)).toEqual(pendingTranscript);
    expect(waitingTranscript({ ...view, agentStatus: "prompting" })).toBeNull();
    expect(waitingTranscript({ ...view, live: { ...view.live, userMessage: { blocks: [] } as never } })).toBeNull();
    expect(waitingTranscript({ ...view, settled: view.live })).toBeNull();
  });

  it("is nothing without a transcript or a view", () => {
    expect(waitingTranscript(initialSessionView("s1"))).toBeNull();
    expect(waitingTranscript(undefined)).toBeNull();
  });
});
