import { ZwiimClientError, type PromptBlock } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  EMPTY_PENDING,
  fileNameOf,
  needsUpload,
  pendingReducer,
  uploadError,
  uploadPending,
  type PendingAction,
  type PendingFile,
  type PendingState,
} from "../src/attachments/pending";

const MB = 1024 * 1024;
const picked = (id: string, size = 1000, name = `${id}.pdf`) => ({
  id,
  name,
  size,
  mime_type: "application/pdf",
  uri: `file:///cache/${name}`,
});
const add = (state: PendingState, ...files: ReturnType<typeof picked>[]) => pendingReducer(state, { type: "add", files });

describe("files chosen for a message", () => {
  it("refuses files over 10 MB and more than 10 per message, keeping the rest", () => {
    let s = add(EMPTY_PENDING, picked("a"), picked("big", 10 * MB + 1, "big.xlsx"));
    expect(s.files.map((f) => f.id)).toEqual(["a"]);
    expect(s.errors).toEqual(["big.xlsx is larger than 10 MB."]);
    s = add(s, ...Array.from({ length: 10 }, (_, i) => picked(`f${i}`)));
    expect(s.files).toHaveLength(10);
    expect(s.errors).toEqual(["Up to 10 files per message."]);
    expect(s.files.every((f) => f.status === "ready")).toBe(true);
  });

  it("removing a file clears the refusals; clear empties it", () => {
    let s = add(EMPTY_PENDING, picked("a"), picked("big", 11 * MB));
    s = pendingReducer(s, { type: "remove", id: "a" });
    expect(s).toEqual({ files: [], errors: [] });
    s = pendingReducer(add(s, picked("b")), { type: "clear" });
    expect(s).toBe(EMPTY_PENDING);
  });

  it("uploads again into another workspace, or after the Host lost them", () => {
    let s = add(EMPTY_PENDING, picked("a"));
    s = pendingReducer(s, { type: "uploaded", id: "a", attachment_id: "att-a", workspace_id: "fix" });
    const a = s.files[0]!;
    expect(needsUpload(a, "fix")).toBe(false);
    expect(needsUpload(a, "other")).toBe(true);
    s = pendingReducer(s, { type: "forget_uploads" });
    expect(s.files[0]).toMatchObject({ status: "ready", attachment_id: undefined });
    expect(needsUpload(s.files[0]!, "fix")).toBe(true);
  });
});

describe("uploading before a send", () => {
  /** Runs `uploadPending` against a reducer, as the hook does. */
  async function run(start: PendingState, put: (f: PendingFile) => Promise<string>) {
    let state = start;
    const puts: string[] = [];
    const dispatch = (a: PendingAction) => (state = pendingReducer(state, a));
    const blocks = await uploadPending(
      start.files,
      "fix",
      (f) => {
        puts.push(f.id);
        return put(f);
      },
      dispatch,
      (e) => uploadError(e, "Studio Mac"),
    );
    return { blocks, state, puts };
  }

  it("uploads each file in order and names them in the message", async () => {
    const { blocks, state } = await run(add(EMPTY_PENDING, picked("a"), picked("b")), async (f) => `att-${f.id}`);
    expect(blocks).toEqual<PromptBlock[]>([
      { type: "attachment", attachment_id: "att-a" },
      { type: "attachment", attachment_id: "att-b" },
    ]);
    expect(state.files.map((f) => f.status)).toEqual(["uploaded", "uploaded"]);
  });

  it("stops the send when one fails, then retries only that one", async () => {
    const first = await run(add(EMPTY_PENDING, picked("a"), picked("b")), async (f) => {
      if (f.id === "b") throw new ZwiimClientError("network", "offline", 0);
      return `att-${f.id}`;
    });
    expect(first.blocks).toBeNull();
    expect(first.state.files[1]).toMatchObject({ status: "failed", error: "Not connected to Studio Mac." });
    const second = await run(first.state, async (f) => `att-${f.id}-2`);
    expect(second.puts).toEqual(["b"]);
    expect(second.blocks).toEqual([
      { type: "attachment", attachment_id: "att-a" },
      { type: "attachment", attachment_id: "att-b-2" },
    ]);
  });
});

describe("naming and errors", () => {
  it("names a picked file by what the picker said, else its URI", () => {
    expect(fileNameOf("file:///x/abc", "  Q3 report.xlsx ")).toBe("Q3 report.xlsx");
    expect(fileNameOf("file:///cache/My%20Doc.docx?x=1", null)).toBe("My Doc.docx");
    expect(fileNameOf("file:///cache/bad%E0.pdf")).toBe("bad%E0.pdf");
    expect(fileNameOf("", "")).toBe("File");
  });

  it("says why an upload failed", () => {
    const e = (code: string) => new ZwiimClientError(code, "x", 400);
    expect(uploadError(e("attachment_too_large"), "Mac")).toBe("Larger than 10 MB.");
    expect(uploadError(e("timeout"), "Mac")).toBe("Not connected to Mac.");
    expect(uploadError(e("workspace_removed"), "Mac")).toBe("That worktree is gone.");
    expect(uploadError(e("not_found"), "Mac")).toBe("That worktree is gone.");
    expect(uploadError(new Error("disk"), "Mac")).toBe("Couldn't upload it.");
  });
});
