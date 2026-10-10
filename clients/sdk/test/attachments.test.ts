import { describe, expect, it, vi } from "vitest";
import {
  ATTACHMENT_MAX_BYTES,
  CLIENT_CAPABILITIES,
  MUTATING_OPS,
  UPLOAD_TIMEOUT_MS,
  attachmentKind,
  checkAttachments,
  formatSize,
  uploadAttachment,
} from "../src";

const file = (name: string, size = 1) => ({ name, size });

describe("checkAttachments", () => {
  it("takes files up to 10 MB", () => {
    const { accepted, errors } = checkAttachments(0, [file("a.pdf", ATTACHMENT_MAX_BYTES)]);
    expect(accepted.map((f) => f.name)).toEqual(["a.pdf"]);
    expect(errors).toEqual([]);
  });

  it("refuses a file over 10 MB by name", () => {
    const { accepted, errors } = checkAttachments(0, [
      file("big.csv", ATTACHMENT_MAX_BYTES + 1),
      file("ok.txt"),
    ]);
    expect(accepted.map((f) => f.name)).toEqual(["ok.txt"]);
    expect(errors).toEqual(["big.csv is larger than 10 MB."]);
  });

  it("stops at ten files per message, saying so once", () => {
    const incoming = Array.from({ length: 5 }, (_, i) => file(`f${i}.txt`));
    const { accepted, errors } = checkAttachments(8, incoming);
    expect(accepted.map((f) => f.name)).toEqual(["f0.txt", "f1.txt"]);
    expect(errors).toEqual(["Up to 10 files per message."]);
  });
});

describe("formatSize", () => {
  it.each([
    [12, "12 B"],
    [340 * 1024, "340 KB"],
    [2.1 * 1024 * 1024, "2.1 MB"],
    [10 * 1024 * 1024, "10 MB"],
  ])("%d → %s", (bytes, text) => expect(formatSize(bytes)).toBe(text));
});

describe("attachmentKind", () => {
  it.each([
    ["report.PDF", undefined, "pdf"],
    ["data.xlsx", undefined, "spreadsheet"],
    ["notes.docx", undefined, "document"],
    ["site.html", undefined, "code"],
    ["bundle.zip", undefined, "archive"],
    ["photo.HEIC", undefined, "image"],
    ["shot.avif", undefined, "image"],
    ["paste", "image/png", "image"],
    ["README", "text/plain", "text"],
    ["blob", "application/octet-stream", "file"],
  ])("%s (%s) → %s", (name, mime, kind) => expect(attachmentKind(name, mime)).toBe(kind));
});

describe("uploadAttachment", () => {
  it("uploads with the long timeout, never retried", async () => {
    const request = vi.fn().mockResolvedValue({ attachment_id: "k3F9a2Qx" });
    await uploadAttachment({ request } as never, "w1", "a.pdf", "QUJD");
    expect(request).toHaveBeenCalledWith(
      "attachments.upload",
      { workspace_id: "w1", name: "a.pdf", data: "QUJD" },
      { timeoutMs: UPLOAD_TIMEOUT_MS },
    );
    expect(MUTATING_OPS.has("attachments.upload")).toBe(true);
    expect(CLIENT_CAPABILITIES).toContain("attachments");
  });
});
