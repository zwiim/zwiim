// Browse Mac (DESIGN.md, New workspace): the host user's home folder, one
// level at a time, named by segments under `~`. Never a host path.

import { isClientError } from "@zwiim/sdk";
import type { Problem } from "./model";

/** "~" or "~/Projects/zwiim". */
export function homeText(path: readonly string[]): string {
  return ["~", ...path].join("/");
}

/** `~` itself can't be a workspace. */
export function canAdd(path: readonly string[]): boolean {
  return path.length > 0;
}

export function addLabel(path: readonly string[]): string {
  return `Add ${homeText(path)}`;
}

export function upPath(path: readonly string[]): string[] {
  return path.slice(0, -1);
}

/** Whether two paths name the same folder. */
export function samePath(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((name, i) => name === b[i]);
}

/** The folder a path names, in words. */
export function folderName(path: readonly string[]): string {
  return path[path.length - 1] ?? "your home folder";
}

function notConnected(e: unknown): boolean {
  return isClientError(e, "network") || isClientError(e, "timeout") || isClientError(e, "unknown_outcome");
}

/** Why a folder didn't open. */
export function browseProblem(e: unknown, path: readonly string[], hostName: string): Problem {
  const folder = folderName(path);
  if (notConnected(e)) return { title: `Not connected to ${hostName}`, body: "Try again when it's back." };
  if (isClientError(e, "permission_denied")) {
    return { title: `Zwiim can't open ${folder} on ${hostName}.`, body: `Allow it in ${hostName}'s privacy settings, or choose another folder.` };
  }
  if (isClientError(e, "not_found")) return { title: `${folder} is gone.`, body: `${hostName} no longer has it.` };
  return { title: `Couldn't open ${folder}`, body: "Try again." };
}

/** Why adding a folder didn't work. */
export function addProblem(e: unknown, path: readonly string[], hostName: string): string {
  const folder = folderName(path);
  if (notConnected(e)) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "permission_denied")) return `Zwiim can't open ${folder} on ${hostName}.`;
  if (isClientError(e, "not_found")) return `${folder} is gone.`;
  if (isClientError(e, "repository_init_failed")) return `Couldn't make ${folder} a Git repository on ${hostName}.`;
  return `Couldn't add ${folder}. Try again.`;
}

/** A name the host would refuse, in words; null when it will try. Empty names just can't be sent. */
export function folderNameProblem(name: string): string | null {
  const n = name.trim();
  if (!n) return null;
  if (n.startsWith(".")) return "Folder names can't start with a dot.";
  if (/[\/\\\0]/.test(n)) return "Folder names can't contain slashes.";
  if (n.length > 255) return "That name is too long.";
  return null;
}

/** Whether Create can be pressed for `name`. */
export function canCreateFolder(name: string): boolean {
  return name.trim() !== "" && folderNameProblem(name) === null;
}

/** Why New folder didn't work. */
export function createFolderProblem(e: unknown, path: readonly string[], name: string, hostName: string): string {
  const folder = folderName(path);
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (notConnected(e)) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "already_exists")) return `There's already something named ${name.trim()} in ${homeText(path)}.`;
  if (isClientError(e, "permission_denied")) return `Zwiim can't create folders in ${folder} on ${hostName}.`;
  if (isClientError(e, "not_found")) return `${folder} is gone.`;
  if (isClientError(e, "invalid_input")) return "Use a plain folder name: no slashes, not starting with a dot.";
  return "Couldn't create the folder. Try again.";
}
