import { describe, expect, it } from "vitest";
import { ZwiimClientError, type FileReader } from "@zwiim/sdk";
import { answerPreview, type PreviewAnswer } from "../src";

const b64 = (s: string) => btoa(s);
function host(files: Record<string, string>, ranges = true) {
  const calls: any[] = [];
  const client: FileReader = {
    state: { compat: { status: "compatible", capabilities: ranges ? ["file_ranges"] : [] } } as any,
    request: (async (_op: string, p: any) => {
      calls.push(p);
      const body = files[p.path];
      if (body === undefined) throw new ZwiimClientError("not_found", "File not found", 404);
      if (body === "!remote") throw new ZwiimClientError("file_too_large_for_remote", "big", 413, { size: 1 });
      if (body === "!down") throw new ZwiimClientError("network", "offline");
      const offset = p.offset ?? 0;
      const end = p.length === undefined ? body.length : Math.min(body.length, offset + p.length);
      return { name: p.path.split("/").pop(), mime_type: "text/html", size: body.length, offset, data: b64(body.slice(offset, end)) };
    }) as any,
  };
  return { client, calls };
}
async function text(a: PreviewAnswer) {
  if (a.status !== 200) return a;
  let s = "";
  for await (const c of a.body) s += new TextDecoder().decode(c);
  return { status: 200, mime_type: a.mime_type, size: a.size, s };
}

describe("answerPreview", () => {
  it("serves workspace files by URL path", async () => {
    const { client } = host({ "docs/report.html": "<h1>hi</h1>", "index.html": "root", "empty.html": "" });
    expect(await text(await answerPreview(client, "w1", "GET", "/docs/report.html?x=1"))).toEqual({
      status: 200, mime_type: "text/html", size: 11, s: "<h1>hi</h1>",
    });
    expect(await text(await answerPreview(client, "w1", "GET", "/"))).toMatchObject({ status: 200, s: "root" });
    expect(await text(await answerPreview(client, "w1", "GET", "/empty.html"))).toMatchObject({ status: 200, size: 0, s: "" });
  });

  it("never asks the Host for a hidden file a page names", async () => {
    const { client, calls } = host({ ".env": "secret", ".git/config": "secret", "a.html": "ok" });
    for (const path of ["/.env", "/%2Eenv", "/.git/config", "/a/../.env", "/.env?x=1"]) {
      expect(await answerPreview(client, "w1", "GET", path, "a.html")).toMatchObject({ status: 404 });
    }
    expect(calls).toEqual([]);
  });

  it("opens a page in a hidden folder with its own files", async () => {
    const { client } = host({ ".site/a.html": "page", ".site/a.css": "css", ".env": "secret" });
    expect(await text(await answerPreview(client, "w1", "GET", "/.site/a.html", ".site/a.html"))).toMatchObject({ status: 200, s: "page" });
    expect(await text(await answerPreview(client, "w1", "GET", "/.site/a.css", ".site/a.html"))).toMatchObject({ status: 200, s: "css" });
    expect(await answerPreview(client, "w1", "GET", "/.env", ".site/a.html")).toMatchObject({ status: 404 });
  });

  it("guards what a page loads, not the page that was opened", async () => {
    const { client, calls } = host({ "a.html": "page", "b.css": "css" });
    (client.state as any).compat.capabilities = ["file_ranges", "files.guard_hidden"];
    await text(await answerPreview(client, "w1", "GET", "/a.html", "a.html"));
    await text(await answerPreview(client, "w1", "GET", "/b.css", "a.html"));
    expect(calls.find((c) => c.path === "a.html")).not.toHaveProperty("guard_hidden");
    expect(calls.find((c) => c.path === "b.css")).toMatchObject({ guard_hidden: true });
  });

  it("answers HEAD with no body", async () => {
    const { client } = host({ "a.html": "abc" });
    expect(await text(await answerPreview(client, "w1", "HEAD", "/a.html"))).toMatchObject({ status: 200, size: 3, s: "" });
  });

  it("refuses other methods, unknown files, remote-too-large and failures", async () => {
    const { client } = host({ "big.pdf": "!remote", "x.html": "!down" });
    expect(await answerPreview(client, "w1", "POST", "/a.html")).toMatchObject({ status: 405 });
    expect(await answerPreview(client, "w1", "GET", "/missing.css")).toMatchObject({ status: 404 });
    expect(await answerPreview(client, "w1", "GET", "/a%2Fb")).toMatchObject({ status: 404 });
    expect(await answerPreview(client, "w1", "GET", "/big.pdf")).toMatchObject({ status: 413 });
    expect(await answerPreview(client, "w1", "GET", "/x.html")).toMatchObject({ status: 502 });
  });

  it("redirects a directory to its index.html", async () => {
    const { client } = host({ "docs/my site/index.html": "home" });
    expect(await answerPreview(client, "w1", "GET", "/docs/my%20site")).toEqual({ status: 301, location: "/docs/my%20site/" });
  });
});
