import { ZwiimClientError } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  RETRY_LABEL,
  RETRY_NEW_CHAT_LABEL,
  RETRY_TARGET,
  retryBusy,
  retryColors,
  retryError,
  retryTitle,
  runRetry,
  type RetryChoice,
} from "../src/sessions/unreachable";

/** What the card's state would hold after each step of a tap. */
function recorder() {
  const seen: { pending: (RetryChoice | null)[]; error: (string | null)[] } = { pending: [], error: [] };
  return { seen, set: { pending: (p: RetryChoice | null) => seen.pending.push(p), error: (e: string | null) => seen.error.push(e) } };
}

describe("can't-reach card", () => {
  it("titles it with the agent and the message, and names the buttons", () => {
    expect(retryTitle("Claude Code", "provider unreachable")).toBe("Claude Code error: provider unreachable");
    expect(RETRY_LABEL).toBe("Retry →");
    expect(RETRY_NEW_CHAT_LABEL).toBe("Retry in new chat →");
  });

  it("gives both buttons a full 44 pt target", () => {
    expect(RETRY_TARGET).toBe(44);
  });

  it("paints the card on surface with an err edge, muted buttons and an err refusal", () => {
    const palette = { surface: "#S", err: "#E", mut: "#M" };
    expect(retryColors(palette)).toEqual({ background: "#S", edge: "#E", action: "#M", refusal: "#E" });
  });

  it("disables both buttons while an op is pending and while offline", () => {
    expect(retryBusy(null, true)).toBe(false);
    expect(retryBusy("retry", true)).toBe(true);
    expect(retryBusy("new_chat", true)).toBe(true);
    expect(retryBusy(null, false)).toBe(true);
  });

  it("holds pending after the op answers: the card is up until the new turn clears it", async () => {
    const { seen, set } = recorder();
    let release!: () => void;
    const op = new Promise<void>((r) => (release = r));
    const done = runRetry("retry", () => op, set, { hostName: "Mac" });
    expect(seen.pending).toEqual(["retry"]);
    release();
    await done;
    expect(seen.pending).toEqual(["retry"]);
    expect(seen.error).toEqual([null]);
  });

  it("hands the op's result on after the try, so a throw there is not a failed retry", async () => {
    const { seen, set } = recorder();
    const after = (id: string) => {
      throw new Error(`navigate to ${id} failed`);
    };
    await expect(runRetry("new_chat", async () => "s2", set, { hostName: "Mac", after })).rejects.toThrow("navigate to s2 failed");
    expect(seen.error).toEqual([null]);
    expect(seen.pending).toEqual(["new_chat"]);
  });

  it("does not hand anything on when the op is refused", async () => {
    const { seen, set } = recorder();
    let called = false;
    await runRetry("new_chat", () => Promise.reject(new ZwiimClientError("turn_in_progress", "x", 409)), set, {
      hostName: "Mac",
      after: () => {
        called = true;
      },
    });
    expect(called).toBe(false);
    expect(seen.pending).toEqual(["new_chat", null]);
  });

  it("shows a refusal in err text and lets the user try again", async () => {
    const { seen, set } = recorder();
    await runRetry("new_chat", () => Promise.reject(new ZwiimClientError("nothing_to_retry", "x", 409)), set, { hostName: "Mac" });
    expect(seen.error).toEqual([null, "There is nothing to retry."]);
    expect(seen.pending).toEqual(["new_chat", null]);
  });

  it("names the Host in the connection copy, as the rest of mobile does", () => {
    expect(retryError(new ZwiimClientError("network", "x"), "Ashish's Mac")).toBe("Not connected to Ashish's Mac. Try again when it's back.");
    expect(retryError(new ZwiimClientError("timeout", "x"), "Ashish's Mac")).toBe("Not connected to Ashish's Mac. Try again when it's back.");
    expect(retryError(new ZwiimClientError("unknown_outcome", "lost"), "Ashish's Mac")).toBe(
      "The connection to Ashish's Mac dropped before it answered. The retry may or may not have been applied. Check the session before trying again.",
    );
  });

  it("words each refusal, and an unknown outcome is never retried by the card", async () => {
    expect(retryError(new ZwiimClientError("turn_in_progress", "x", 409), "Mac")).toMatch(/still working/);
    expect(retryError(new ZwiimClientError("unknown_outcome", "lost"), "Mac")).toMatch(/may or may not have been applied/);
    expect(retryError(new Error("boom"), "Mac")).toBe("Couldn't retry. Try again.");
    let calls = 0;
    const { seen, set } = recorder();
    await runRetry("retry", async () => {
      calls++;
      throw new ZwiimClientError("unknown_outcome", "lost");
    }, set, { hostName: "Mac" });
    expect(calls).toBe(1);
    expect(seen.error.at(-1)).toMatch(/may or may not have been applied/);
  });
});
