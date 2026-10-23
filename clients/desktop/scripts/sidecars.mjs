// Build the host binaries in release and stage them where the bundler picks
// them up as Tauri sidecars: `src-tauri/binaries/<name>-<target triple>`.
// The bundle places them next to the Desktop executable, which is where
// the Desktop looks for `zwiim-host` and the host looks for `zwiim-mcp`
// and `gh` (the pinned GitHub CLI, see scripts/gh/fetch.mjs).
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { fetchGh } from "../../../scripts/gh/fetch.mjs";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(desktop, "../..");
const triple = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (\S+)$/m)[1];
const ext = process.platform === "win32" ? ".exe" : "";

execFileSync("cargo", ["build", "--release", "-p", "zwiim-host", "--bin", "zwiim-host", "--bin", "zwiim-mcp"], {
  cwd: root,
  stdio: "inherit",
});
const out = path.join(desktop, "src-tauri/binaries");
fs.mkdirSync(out, { recursive: true });
for (const name of ["zwiim-host", "zwiim-mcp"]) {
  const dest = path.join(out, `${name}-${triple}${ext}`);
  fs.copyFileSync(path.join(root, "target/release", name + ext), dest);
  console.log(`staged ${path.relative(desktop, dest)}`);
}
const gh = path.join(out, `gh-${triple}${ext}`);
fs.copyFileSync((await fetchGh(triple)).bin, gh);
fs.chmodSync(gh, 0o755);
console.log(`staged ${path.relative(desktop, gh)}`);
