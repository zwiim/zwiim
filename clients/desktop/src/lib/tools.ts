// A tool call read the way a person reads it: what it does (Run, Read,
// Edit…) and what it works on (a command, a path, a query). The same reading
// as the mobile transcript (`clients/mobile/src/sessions/transcript.ts`).

import { firstQuestion, zwiimTool, type ZwiimToolGroup } from "@zwiim/sdk";
import { editChanges, parseCodexPatch, type FileDiff } from "@/lib/diff";

/** The most of a tool's input or output the transcript shows. More (an
 * image's base64, a huge log) takes seconds to lay out and is not read. */
const SHOWN_CHARS = 20_000;

/** A tool's text cut to what the transcript shows, saying how much is left out. */
export function clipToolText(text: string): string {
  if (text.length <= SHOWN_CHARS) return text;
  return `${text.slice(0, SHOWN_CHARS)}\n… ${(text.length - SHOWN_CHARS).toLocaleString("en-US")} more characters`;
}

/** What a tool does, from the agent's kind for a live call. */
const KIND_VERBS: Record<string, string> = {
  read: "Read",
  edit: "Edit",
  delete: "Delete",
  move: "Move",
  search: "Search",
  execute: "Run",
  fetch: "Fetch",
  think: "Think",
};

/** What a tool does, from its name in a transcript. */
const NAME_VERBS: Record<string, string> = {
  bash: "Run",
  shell: "Run",
  exec_command: "Run",
  read: "Read",
  edit: "Edit",
  multiedit: "Edit",
  write: "Write",
  grep: "Search",
  glob: "Search",
  websearch: "Search",
  webfetch: "Fetch",
  task: "Agent",
  agent: "Agent",
  todowrite: "Todos",
  // Claude titles it so live.
  exitplanmode: "Exited Plan Mode",
  // Codex
  apply_patch: "Edit",
  write_stdin: "Input",
  codex_script: "Script",
  request_user_input: "Ask",
  view_image: "View",
};

const TARGET_KEYS = [
  "command",
  "cmd",
  "pattern",
  "query",
  "url",
  "file_path",
  "notebook_path",
  "path",
  "description",
  "session_command",
];
const PATH_KEYS = new Set(["file_path", "notebook_path", "path"]);
/** Verbs whose target is a file. */
const FILE_VERBS = new Set(["Read", "Edit", "Write", "Delete"]);

export interface ToolSummary {
  /** Run, Read, Edit…; the tool's own name when it has no common verb. */
  verb: string;
  /** What it works on, on one line; empty when nothing says. */
  target: string;
  /** Its input as a person reads it: the whole command, or what the target leaves out. */
  input: string | null;
  /** The file it works on, when it works on one. */
  path?: string | null;
  /** The changes an edit makes, a file each, read from its input. */
  changes?: FileDiff[] | null;
  /** Which of Zwiim's own tools it is, for its icon; absent for any other tool. */
  group?: ZwiimToolGroup;
}

/** A title without the backticks agents wrap commands in. */
const unquote = (title: string) => {
  const t = title.trim();
  return t.length > 1 && t.startsWith("`") && t.endsWith("`") ? t.slice(1, -1) : t;
};

/**
 * A command given as one double-quoted shell word — Codex's `zsh -lc "…"`
 * script once its shell is dropped — as the script itself.
 */
const shellUnquote = (command: string) => {
  const m = /^"((?:[^"\\]|\\.)*)"$/s.exec(command);
  return m ? m[1]!.replace(/\\(["\\$`])/g, "$1") : command;
};

const firstLine = (text: string) => text.trim().split("\n")[0]!.trim();

const str = (v: unknown) => (typeof v === "string" ? v : null);

/** The keys a change already shows. */
const CHANGE_KEYS = new Set(["old_string", "new_string", "edits", "content", "replace_all"]);

/** Codex's apply_patch: its patch is its input, and says which files it adds, changes or deletes. */
function patchTool(input: string | null): ToolSummary | null {
  const files = parseCodexPatch(input ?? "");
  if (!files) return null;
  const ops = new Set(files.map((f) => f.op));
  const verb = ops.size > 1 ? "Edit" : ops.has("add") ? "Write" : ops.has("delete") ? "Delete" : "Edit";
  const path = files[0]!.diff.path;
  // A deleted file has no lines to show.
  const changes = files.map((f) => f.diff).filter((d) => d.lines.length > 0);
  return { verb, target: path ?? "", input: null, path, changes: changes.length > 0 ? changes : null };
}

/** What a Codex or question tool works on, where its input does not name it under a common key. */
function codexTarget(name: string, args: Record<string, unknown>): string | null {
  if (name === "request_user_input" || name.endsWith("ask_user_question")) return firstQuestion(args);
  if (name === "codex_script") return str(args.source);
  return null;
}

/** A call from a transcript: the tool's name and its input as JSON. */
export function historyTool(name: string, input: string | null): ToolSummary {
  const summary = readHistoryTool(name, input);
  const own = zwiimTool(name);
  return own ? { ...summary, group: own.group } : summary;
}

function readHistoryTool(name: string, input: string | null): ToolSummary {
  if (name === "context_compaction") return { verb: "Chat compacted", target: "", input: null };
  if (name === "apply_patch") {
    const patch = patchTool(input);
    if (patch) return patch;
  }
  let args: Record<string, unknown> | null = null;
  try {
    const parsed: unknown = input ? JSON.parse(input) : null;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) args = parsed as Record<string, unknown>;
  } catch {
    // Not JSON (or cut short): shown as it is.
  }
  const verb = zwiimTool(name)?.label ?? NAME_VERBS[name.toLowerCase()] ?? name;
  if (!args) {
    const target = firstLine(input ?? "");
    return { verb, target, input: input?.trim() === target ? null : input };
  }
  const value = (key: string) => {
    const v = args[key];
    return Array.isArray(v) ? v.join(" ") : typeof v === "string" ? v : null;
  };
  const key = TARGET_KEYS.find((k) => value(k)?.trim());
  const named = codexTarget(name, args);
  const target = firstLine(named ?? (key ? value(key)! : ""));
  const path = key && PATH_KEYS.has(key) ? value(key) : null;
  const changes = editChanges(name, input);
  const change = changes?.[0] ?? null;
  const command = value("command") ?? value("cmd");
  if (command !== null) return { verb, target, input: command, path, changes };
  // A script reads in full; the rest of the input, one argument a line.
  const script = name === "codex_script" ? named : null;
  const rest = Object.entries(args).filter(
    ([k]) => k !== key && !(change && CHANGE_KEYS.has(k)) && !(script !== null && k === "source"),
  );
  const shown = rest.map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v, null, 2)}`).join("\n");
  const all = [script, shown].filter(Boolean).join("\n\n");
  return { verb, target, input: all || null, path, changes };
}

/**
 * Titles Codex gives its live calls, in its own words: `Read file 'a.ts'`,
 * `List files in 'src'`, `Search for 'x' in src`.
 */
const CODEX_TITLES: [RegExp, string][] = [
  [/^Read file '(.+)'$/, "Read"],
  [/^List files(?: in '(.+)')?$/, "List"],
  [/^Search (?:for )?(.+)$/, "Search"],
  [/^View Image (.+)$/, "View"],
  [/^Compact conversation$/, "Chat compacted"],
];

/** A live call: the agent's title ("Read src/a.ts", "`ls -la`") and kind. */
export function liveTool(title: string | null, kind: string | null | undefined): ToolSummary {
  const t = unquote(title ?? "");
  const own = zwiimTool(t);
  if (own) return { verb: own.label, target: "", input: null, group: own.group };
  const summary = (verb: string, whole: string): ToolSummary => ({
    verb,
    target: firstLine(whole),
    // A command reads in full on expand; the row shows its first line.
    input: verb === "Run" ? whole : null,
    path: FILE_VERBS.has(verb) ? firstLine(whole) : null,
  });
  // A command's title is the command, whatever word it starts with; a Codex
  // MCP tool's is `mcp.<server>.<tool>`.
  if (kind === "execute") {
    const mcp = /^mcp\.([^.\s]+)\.(\S+)$/.exec(t);
    return mcp ? { verb: mcp[2]!, target: mcp[1]!, input: null } : summary("Run", shellUnquote(t));
  }
  for (const [pattern, verb] of CODEX_TITLES) {
    const m = pattern.exec(t);
    if (m) return summary(verb, m[1] ?? "");
  }
  // The title often starts with the verb already ("Read src/a.ts", "Write a.ts").
  const [word = "", ...rest] = t.split(" ");
  const named = NAME_VERBS[word.toLowerCase()] ?? KIND_VERBS[word.toLowerCase()];
  if (named && named.toLowerCase() === word.toLowerCase() && rest.length > 0) {
    return summary(named, unquote(rest.join(" ")));
  }
  const verb = kind ? KIND_VERBS[kind] : undefined;
  if (!verb) return { verb: t ? firstLine(t) : "Tool call", target: "", input: null };
  // Codex titles an edit "Editing files"; its content names them.
  if (verb === "Edit" && /^Edit(?:ing)? files$/.test(t)) return { verb, target: "", input: null };
  return summary(verb, t);
}
