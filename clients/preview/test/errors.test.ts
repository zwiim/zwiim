import { describe, expect, it } from "vitest";
import { ZwiimClientError } from "@zwiim/sdk";
import { previewErrorMessage, previewStatus } from "../src";

describe("preview errors", () => {
  it("speaks the spec's copy", () => {
    const err = (code: any, details?: any) => new ZwiimClientError(code, "x", 400, details);
    expect(previewErrorMessage(err("not_found"), "a.pdf")).toBe("This file no longer exists.");
    expect(previewErrorMessage(err("workspace_removed"), "a.pdf")).toBe("This file no longer exists.");
    expect(previewErrorMessage(err("file_too_large_for_remote", { size: 412 * 1024 * 1024 }), "a.pdf")).toBe(
      "Too large to preview over a remote connection (412 MB). It opens on the same network.",
    );
    expect(previewErrorMessage(err("file_too_large"), "a.pdf")).toBe("Too large to preview.");
    expect(previewErrorMessage(err("network"), "a.pdf")).toBeNull();
    expect(previewErrorMessage(Object.assign(new Error("pw"), { name: "PasswordException" }), "a.pdf")).toBe(
      "Couldn't preview a.pdf. It's password-protected.",
    );
    expect(previewErrorMessage(new Error("bad xref"), "a.pdf")).toBe(
      "Couldn't preview a.pdf. The file looks damaged or isn't supported.",
    );
  });

  it("maps errors to HTTP statuses", () => {
    expect(previewStatus(new ZwiimClientError("not_found", "x"))).toBe(404);
    expect(previewStatus(new ZwiimClientError("workspace_removed", "x"))).toBe(404);
    expect(previewStatus(new ZwiimClientError("file_too_large", "x"))).toBe(413);
    expect(previewStatus(new ZwiimClientError("file_too_large_for_remote", "x"))).toBe(413);
    expect(previewStatus(new ZwiimClientError("network", "x"))).toBe(502);
    expect(previewStatus(new Error("x"))).toBe(502);
  });
});
