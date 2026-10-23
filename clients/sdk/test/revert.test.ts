import { describe, expect, it } from "vitest";
import { CLIENT_CAPABILITIES, evaluateCompatibility } from "../src/client/compat";
import { ZwiimClientError } from "../src/client/errors";
import { HEAD_MOVED_NOTE, offersChatRewind, revertNotes, revertRefusalText } from "../src/client/revert";
import type { HistoryTurn } from "../src/types/protocol";
import { hello } from "./fakes";

const turn = (role: string): HistoryTurn => ({
  turn_id: role,
  role,
  blocks: [],
  timestamp: "2026-01-01T00:00:00Z",
  completed_at: null,
  duration_ms: null,
  model: null,
  usage: null,
});
const compat = (capabilities: readonly string[] = CLIENT_CAPABILITIES) =>
  evaluateCompatibility(hello({ capabilities: [...capabilities] }));
const opts = (can_rewind_chat?: boolean) => ({
  model: null,
  effort: null,
  mode: null,
  ...(can_rewind_chat === undefined ? {} : { can_rewind_chat }),
});

describe("offersChatRewind", () => {
  it("takes the chat back only when the Host and agent can, and a reply comes before the message", () => {
    expect(offersChatRewind(turn("assistant"), opts(true), compat())).toBe(true);
    // Not known yet counts as yes: the Host says so if the agent cannot.
    expect(offersChatRewind(turn("assistant"), opts(), compat())).toBe(true);
    expect(offersChatRewind(turn("assistant"), null, compat())).toBe(true);
    expect(offersChatRewind(turn("assistant"), opts(false), compat())).toBe(false);
    expect(offersChatRewind(turn("assistant"), opts(true), compat(CLIENT_CAPABILITIES.filter((c) => c !== "session.revert.chat")))).toBe(false);
    expect(offersChatRewind(turn("assistant"), opts(true), null)).toBe(false);
    // The first message has nothing before it; a user turn is no reply.
    expect(offersChatRewind(undefined, opts(true), compat())).toBe(false);
    expect(offersChatRewind(null, opts(true), compat())).toBe(false);
    expect(offersChatRewind(turn("user"), opts(true), compat())).toBe(false);
  });
});

describe("revertRefusalText", () => {
  const err = (code: ConstructorParameters<typeof ZwiimClientError>[0], details?: Record<string, unknown>) =>
    new ZwiimClientError(code, "x", 409, details);

  it("says why the Host refused a reset", () => {
    expect(revertRefusalText(err("checkpoint_unavailable"))).toBe("This point can't be restored.");
    expect(revertRefusalText(err("workspace_busy", { live_sessions: 0, terminals: 0, session_ids: [], operation: "merge" }))).toBe(
      "The workspace is busy: a merge is in progress. Finish it, then try again.",
    );
    expect(revertRefusalText(err("workspace_busy", { live_sessions: 0, terminals: 0, session_ids: [], operation: "index_lock" }))).toBe(
      "The workspace is busy: another Git command is running in it. Try again in a moment.",
    );
    expect(revertRefusalText(err("workspace_busy", { live_sessions: 1, terminals: 0, session_ids: ["s2"] }))).toBe(
      "The workspace is busy: another agent is working in it. Try again when it's done.",
    );
    expect(revertRefusalText(err("workspace_busy", { live_sessions: 0, terminals: 0, session_ids: [], operation: "unheard_of" }))).toBe(
      "The workspace is busy: another Git operation is in progress. Finish it, then try again.",
    );
    expect(revertRefusalText(err("turn_in_progress"))).toBe(
      "The agent is still working. Wait for it to finish, or stop it, then try again.",
    );
    expect(revertRefusalText(err("root_workspace"))).toBe("This session ran in the repository folder, so it can't be reset.");
  });

  it("leaves other failures to the app's own wording", () => {
    expect(revertRefusalText(err("network"))).toBeNull();
    expect(revertRefusalText(err("unknown_outcome"))).toBeNull();
    expect(revertRefusalText(new Error("boom"))).toBeNull();
  });
});

describe("revertNotes", () => {
  it("says what a done reset left behind, and nothing when it did all it was asked", () => {
    expect(revertNotes({ files_restored: true, chat_rewound: true, head_moved: false }, { files: true, chat: true })).toEqual([]);
    expect(revertNotes({ files_restored: true, chat_rewound: false, head_moved: false }, { files: true, chat: false })).toEqual([]);
    expect(HEAD_MOVED_NOTE).toBe("Commits made after this point stay; their changes are undone as uncommitted edits.");
    expect(revertNotes({ files_restored: true, chat_rewound: false, head_moved: true }, { files: true, chat: false })).toEqual([HEAD_MOVED_NOTE]);
    expect(revertNotes({ files_restored: true, chat_rewound: false, head_moved: true }, { files: true, chat: true })).toEqual([
      "The files were reset, but the chat could not be taken back. Its later messages stay.",
      HEAD_MOVED_NOTE,
    ]);
    expect(revertNotes({ files_restored: false, chat_rewound: false, head_moved: false }, { files: false, chat: true })).toEqual([
      "The chat could not be taken back. Its later messages stay.",
    ]);
  });
});
