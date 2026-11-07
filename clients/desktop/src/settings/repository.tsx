// The Repository tab: how the host prepares each of a repository's worktrees —
// the scripts it runs around a worktree's life, the ignored files it copies in,
// the variables its agents, terminals and scripts get and where its servers
// open. The host keeps them
// per repository, so a worktree made from a phone is set up the same way as
// one made here.

import { useCallback, useEffect, useState } from "react";
import {
  DEFAULT_FILES_TO_COPY,
  hasCapability,
  MAX_ENV_BYTES,
  MAX_ENV_NAME_BYTES,
  MAX_FILE_PATTERN_BYTES,
  MAX_FILES_TO_COPY,
  MAX_PREVIEW_NAME_BYTES,
  MAX_PREVIEW_URL_BYTES,
  MAX_PREVIEW_URLS,
  MAX_SCRIPT_BYTES,
  type Preview,
  type RepositorySettingKey,
  type RepositorySettings as KeptSettings,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import { ControlSelect } from "@/panels/OptionsBar";

type Values = KeptSettings["values"];

interface ScriptDef {
  key: RepositorySettingKey;
  id: string;
  title: string;
  description: string;
  placeholder: string;
}

const SCRIPTS: readonly ScriptDef[] = [
  {
    key: "scripts.setup",
    id: "script-setup",
    title: "Setup script",
    description:
      "Runs in a new worktree, in a terminal named Setup, which stays open as a shell when it ends. Agents start without waiting for it.",
    placeholder: "npm install",
  },
  {
    key: "scripts.run",
    id: "script-run",
    title: "Run script",
    description:
      "Runs when you press Run in a worktree's terminals, in a terminal named Run; running it again restarts it. Use $ZWIIM_PORT, the first of ten ports each worktree has to itself, so worktrees' servers never clash.",
    placeholder: "npm run dev -- --port $ZWIIM_PORT",
  },
  {
    key: "scripts.archive",
    id: "script-archive",
    title: "Archive script",
    description:
      "Runs in a worktree before it is removed, for up to 5 minutes. The worktree is removed however it ends.",
    placeholder: "docker compose down",
  },
];

const encoder = new TextEncoder();

/** The repositories that can have worktrees, so scripts. */
function repositories(workspaces: readonly WorkspaceInfo[] | null): WorkspaceInfo[] {
  return (workspaces ?? []).filter((w) => w.kind === "root" && !w.removed && w.is_git_repo);
}

export function RepositorySettings({ workspaceId }: { workspaceId?: string | null }) {
  const workspaces = useHost((s) => s.workspaces);
  const repos = repositories(workspaces);
  // The repository of what the window shows, else the first.
  const current = workspaces?.find((w) => w.workspace_id === workspaceId);
  const preferred = current?.kind === "root" ? current.workspace_id : current?.parent_workspace_id;
  const [picked, setPicked] = useState<string | null>(null);
  const repoId =
    repos.find((r) => r.workspace_id === picked)?.workspace_id ??
    repos.find((r) => r.workspace_id === preferred)?.workspace_id ??
    repos[0]?.workspace_id ??
    null;

  if (!repoId) {
    return <p className="text-muted-foreground">Add a Git repository to set up the worktrees made from it.</p>;
  }
  return (
    <>
      <p className="text-muted-foreground">
        How the host prepares each of a repository's worktrees, however the worktree was made.
      </p>
      <div className="flex items-center justify-between gap-3 border-b pb-3">
        <span className="text-ui">Repository</span>
        <ControlSelect
          label="Repository"
          value={repoId}
          onChange={setPicked}
          options={repos.map((r) => ({ value: r.workspace_id, label: r.alias ?? r.name, title: r.display_path ?? undefined }))}
        />
      </div>
      {/* Keyed so a switch drops the other repository's drafts. */}
      <Scripts key={repoId} repoId={repoId} />
    </>
  );
}

const textareaClass =
  "bg-input/30 placeholder:text-muted-foreground min-h-24 w-full resize-y rounded-md border px-2 py-1.5 font-mono text-xs leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-ring/50";

function Scripts({ repoId }: { repoId: string }) {
  const controller = useController();
  const online = useOnline();
  const [values, setValues] = useState<Values | null>(null);
  const [drafts, setDrafts] = useState<Partial<Record<RepositorySettingKey, string>>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const canImport = useHost((s) =>
    hasCapability(s.connection.compat, "repository.settings_import"),
  );

  const load = useCallback((kept: Values) => {
    setValues(kept);
    setDrafts({
      ...Object.fromEntries(SCRIPTS.map((p) => [p.key, text(kept[p.key])])),
      "files.copy": filesToCopy(kept).join("\n"),
      env: formatEnv(kept.env ?? {}),
    });
  }, []);

  useEffect(() => {
    controller.client.request("repository.settings.get", { workspace_id: repoId }).then(
      (s) => load(s.values),
      (e) => setError(errorMessage(e)),
    );
  }, [controller, repoId, load]);

  const importScripts = () => {
    setError(null);
    setNotice(null);
    controller.client
      .request("repository.settings.import", { workspace_id: repoId })
      .then((s) => {
        load(s.values);
        setNotice(
          s.imported.length === 0
            ? "Nothing imported: these scripts are already set."
            : `Imported ${s.imported.length === 1 ? "1 script" : `${s.imported.length} scripts`}. Check them before creating a worktree.`,
        );
      })
      .catch((e) => setError(errorMessage(e)));
  };

  const update = useCallback(
    (patch: Values) => {
      setError(null);
      controller.client
        .request("repository.settings.update", { workspace_id: repoId, values: patch })
        .then((s) => setValues(s.values))
        .catch((e) => setError(errorMessage(e)));
    },
    [controller, repoId],
  );

  const draftOf = (key: RepositorySettingKey) => drafts[key] ?? "";
  const setDraft = (key: RepositorySettingKey, value: string) => setDrafts((d) => ({ ...d, [key]: value }));

  if (!values) {
    return error ? (
      <p className="text-destructive" role="alert">
        {error}
      </p>
    ) : null;
  }

  const patterns = parsePatterns(draftOf("files.copy"));
  const env = parseEnv(draftOf("env"));

  return (
    <>
      {canImport && (
        <div className="flex items-center justify-between gap-3 border-b pb-3">
          <span className="text-muted-foreground">
            {notice ?? "Fill empty scripts from this repository's Conductor settings."}
          </span>
          <Button variant="outline" size="sm" disabled={!online} onClick={importScripts}>
            Import from Conductor
          </Button>
        </div>
      )}
      {SCRIPTS.map((p) => {
        const draft = draftOf(p.key);
        const over = encoder.encode(draft.trim()).length > MAX_SCRIPT_BYTES;
        return (
          <Field key={p.key} id={p.id} title={p.title} description={p.description} problems={over ? [TOO_LONG] : []}>
            <textarea
              id={p.id}
              className={textareaClass}
              placeholder={p.placeholder}
              value={draft}
              disabled={!online}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              aria-invalid={over || undefined}
              onChange={(e) => setDraft(p.key, e.target.value)}
              onBlur={() => {
                const next = draft.trim();
                if (over || next === text(values[p.key])) return;
                update({ [p.key]: next === "" ? null : next });
              }}
            />
          </Field>
        );
      })}
      <Field
        id="files-copy"
        title="Files to copy"
        description="Git-ignored files of the repository folder copied into each new worktree, such as .env files Git leaves out. One .gitignore-style pattern per line; lines starting with # are skipped. Leave it empty to copy nothing."
        problems={patterns.problems}
      >
        <textarea
          id="files-copy"
          className={textareaClass}
          placeholder="Nothing is copied"
          value={draftOf("files.copy")}
          disabled={!online}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          aria-invalid={patterns.problems.length > 0 || undefined}
          onChange={(e) => setDraft("files.copy", e.target.value)}
          onBlur={() => {
            if (patterns.problems.length > 0 || same(patterns.value, filesToCopy(values))) return;
            update({ "files.copy": same(patterns.value, DEFAULT_FILES_TO_COPY) ? null : patterns.value });
          }}
        />
      </Field>
      <EnvField
        kept={values.env ?? {}}
        draft={draftOf("env")}
        parsed={env}
        online={online}
        onDraft={(v) => setDraft("env", v)}
        onSave={() => {
          if (env.problems.length > 0 || same(entries(env.value), entries(values.env ?? {}))) return;
          update({ env: Object.keys(env.value).length === 0 ? null : env.value });
        }}
      />
      <PreviewsField
        kept={values["preview.urls"] ?? []}
        online={online}
        onSave={(previews) => update({ "preview.urls": previews.length === 0 ? null : previews })}
      />
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

const TOO_LONG = "Too long for the host to keep. Shorten it to save.";

function Field({
  id,
  title,
  description,
  problems,
  action,
  children,
}: {
  id: string;
  title: string;
  description: string;
  problems: readonly string[];
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1.5 border-b pb-3" data-setting-id={id}>
      <div className="flex items-center justify-between gap-3">
        <label htmlFor={id} className="text-ui">
          {title}
        </label>
        {action}
      </div>
      <span className="text-muted-foreground">{description}</span>
      {children}
      {problems.length > 0 && (
        <div className="text-destructive" role="alert">
          {problems.map((p) => (
            <p key={p}>{p}</p>
          ))}
        </div>
      )}
    </section>
  );
}

/**
 * The repository's variables, whose values stay hidden until asked for: they
 * are often keys. With none kept yet there is nothing to hide.
 */
function EnvField({
  kept,
  draft,
  parsed,
  online,
  onDraft,
  onSave,
}: {
  kept: Record<string, string>;
  draft: string;
  parsed: Parsed<Record<string, string>>;
  online: boolean;
  onDraft: (value: string) => void;
  onSave: () => void;
}) {
  const names = Object.keys(kept);
  const [shown, setShown] = useState(names.length === 0);
  const description =
    "Set for every agent, terminal and script in the repository's worktrees. The host keeps them in your keychain, not its database. One NAME=value per line, as in a .env file.";
  if (!shown && names.length > 0) {
    return (
      <Field
        id="env-list"
        title="Environment variables"
        description={description}
        problems={[]}
        action={
          <Button size="xs" variant="outline" disabled={!online} onClick={() => setShown(true)}>
            Show and edit
          </Button>
        }
      >
        <ul id="env-list" aria-label="Environment variables" className="flex flex-col gap-0.5 font-mono text-xs">
          {names.map((n) => (
            <li key={n}>
              {n}=<span className="text-muted-foreground">••••••</span>
            </li>
          ))}
        </ul>
      </Field>
    );
  }
  return (
    <Field
      id="env"
      title="Environment variables"
      description={description}
      problems={parsed.problems}
      action={
        names.length > 0 && (
          <Button
            size="xs"
            variant="outline"
            // Hiding leaves the field, so it saves first.
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              onSave();
              if (parsed.problems.length === 0) setShown(false);
            }}
          >
            Hide values
          </Button>
        )
      }
    >
      <textarea
        id="env"
        className={textareaClass}
        placeholder={"API_URL=http://localhost:4000\nDATABASE_URL=postgres://localhost/dev"}
        value={draft}
        disabled={!online}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        autoComplete="off"
        aria-invalid={parsed.problems.length > 0 || undefined}
        onChange={(e) => onDraft(e.target.value)}
        onBlur={onSave}
      />
    </Field>
  );
}

const inputClass =
  "bg-input/30 placeholder:text-muted-foreground h-7 rounded-md border px-2 font-mono text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50";

/**
 * Where Open goes: a row per page, its name and its address. A row is saved
 * when focus leaves it, so moving from its name to its address saves nothing yet.
 */
function PreviewsField({
  kept,
  online,
  onSave,
}: {
  kept: readonly Preview[];
  online: boolean;
  onSave: (previews: Preview[]) => void;
}) {
  const [rows, setRows] = useState<PreviewRow[]>(() =>
    kept.length > 0 ? kept.map((p) => ({ name: p.name ?? "", url: p.url })) : [BLANK_PREVIEW],
  );
  // What is wrong, as of the last time focus left a row: not while one is typed.
  const [checked, setChecked] = useState<{ problems: string[]; bad: number[] }>({ problems: [], bad: [] });
  const save = (next: PreviewRow[]) => {
    const c = checkPreviews(next);
    setChecked(c);
    if (c.problems.length > 0 || same(c.value, kept)) return;
    onSave(c.value);
  };
  const edit = (i: number, patch: Partial<PreviewRow>) =>
    setRows((r) => r.map((row, j) => (j === i ? { ...row, ...patch } : row)));
  const remove = (i: number) => {
    const next = rows.filter((_, j) => j !== i);
    setRows(next.length > 0 ? next : [BLANK_PREVIEW]);
    save(next);
  };
  return (
    <Field
      id="preview-urls"
      title="Preview URLs"
      description="The pages Open in a worktree's terminals goes to, the first by default. Use $ZWIIM_PORT for the worktree's own port. With none, Open goes to the servers its terminals print."
      problems={checked.problems}
    >
      <ul className="flex flex-col gap-1.5" aria-label="Preview URLs">
        {rows.map((row, i) => (
          <li
            key={i}
            className="flex items-center gap-1.5"
            onBlur={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget)) save(rows);
            }}
          >
            <input
              aria-label={`Name of preview ${i + 1}`}
              className={cn(inputClass, "w-28 shrink-0")}
              placeholder="Name"
              value={row.name}
              disabled={!online}
              spellCheck={false}
              onChange={(e) => edit(i, { name: e.target.value })}
            />
            <input
              id={i === 0 ? "preview-urls" : undefined}
              aria-label={`URL of preview ${i + 1}`}
              className={cn(inputClass, "min-w-0 flex-1")}
              placeholder="http://localhost:$ZWIIM_PORT"
              value={row.url}
              disabled={!online}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              aria-invalid={checked.bad.includes(i) || undefined}
              onChange={(e) => edit(i, { url: e.target.value })}
            />
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={`Remove preview ${i + 1}`}
              disabled={!online || (rows.length === 1 && row.name === "" && row.url === "")}
              onClick={() => remove(i)}
            >
              <X className="size-3.5" />
            </Button>
          </li>
        ))}
      </ul>
      <Button
        size="xs"
        variant="outline"
        className="self-start"
        disabled={!online || rows.length >= MAX_PREVIEW_URLS}
        onClick={() => setRows((r) => [...r, BLANK_PREVIEW])}
      >
        <Plus />
        Add URL
      </Button>
    </Field>
  );
}

function text(v: Values[string] | undefined): string {
  return typeof v === "string" ? v : "";
}

function filesToCopy(values: Values): readonly string[] {
  return values["files.copy"] ?? DEFAULT_FILES_TO_COPY;
}

function same(a: readonly unknown[], b: readonly unknown[]): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function entries(env: Record<string, string>): [string, string][] {
  return Object.entries(env).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

interface Parsed<T> {
  value: T;
  /** Why the host would refuse it, so it is not sent. */
  problems: string[];
}

/** One pattern per line; blank lines and `#` comments are dropped. */
export function parsePatterns(draft: string): Parsed<string[]> {
  const value = draft
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "" && !l.startsWith("#"));
  const problems: string[] = [];
  if (value.length > MAX_FILES_TO_COPY) problems.push(`At most ${MAX_FILES_TO_COPY} patterns.`);
  const long = value.find((p) => encoder.encode(p).length > MAX_FILE_PATTERN_BYTES);
  if (long) problems.push(`Pattern too long: ${long.slice(0, 40)}…`);
  return { value, problems };
}

const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * `.env` lines: blank lines and `#` comments are skipped, `export ` is
 * allowed, the value is everything after the first `=`, trimmed, and one pair
 * of matching quotes around it is removed. A later line wins over an earlier
 * one with the same name.
 */
export function parseEnv(draft: string): Parsed<Record<string, string>> {
  const value: Record<string, string> = {};
  const problems: string[] = [];
  draft.split("\n").forEach((raw, i) => {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) return;
    const eq = line.indexOf("=");
    if (eq < 0) {
      problems.push(`Line ${i + 1}: expected NAME=value.`);
      return;
    }
    const name = line
      .slice(0, eq)
      .replace(/^export\s+/, "")
      .trim();
    if (!ENV_NAME.test(name) || name.length > MAX_ENV_NAME_BYTES) {
      problems.push(`Line ${i + 1}: ${name || "(empty)"} is not a variable name.`);
    } else if (name.toUpperCase().startsWith("ZWIIM_")) {
      problems.push(`Line ${i + 1}: names starting with ZWIIM_ are the host's own.`);
    } else {
      value[name] = unquote(line.slice(eq + 1).trim());
    }
  });
  const bytes = Object.entries(value).reduce((n, [k, v]) => n + encoder.encode(k).length + encoder.encode(v).length, 0);
  if (bytes > MAX_ENV_BYTES) problems.push(TOO_LONG);
  return { value, problems };
}

function unquote(v: string): string {
  return v.length >= 2 && (v[0] === '"' || v[0] === "'") && v.at(-1) === v[0] ? v.slice(1, -1) : v;
}

const WEB_URL = /^https?:\/\//i;
/** `$((ZWIIM_PORT + n))`, which the Host turns into a port, spaces and all. */
const PORT_SUM = /\$\(\(\s*ZWIIM_PORT\s*(?:\+\s*\d\s*)?\)\)/g;

export interface PreviewRow {
  name: string;
  url: string;
}

const BLANK_PREVIEW: PreviewRow = { name: "", url: "" };

/**
 * The previews the rows hold, in order. Empty rows are skipped, and an address
 * without `http://` or `https://` gets `http://`, as a local server's usually wants.
 */
export function checkPreviews(rows: readonly PreviewRow[]): Parsed<Preview[]> & { bad: number[] } {
  const value: Preview[] = [];
  const problems: string[] = [];
  const bad: number[] = [];
  rows.forEach((row, i) => {
    const name = row.name.trim();
    let url = row.url.trim();
    if (name === "" && url === "") return;
    const problem = (p: string) => {
      problems.push(`${name || `Row ${i + 1}`}: ${p}`);
      bad.push(i);
    };
    if (url === "") return problem("add its URL.");
    if (!WEB_URL.test(url)) url = `http://${url}`;
    if (/\s/.test(url.replace(PORT_SUM, ""))) problem("a URL has no spaces.");
    else if (encoder.encode(url).length > MAX_PREVIEW_URL_BYTES) problem("URL too long.");
    else if (encoder.encode(name).length > MAX_PREVIEW_NAME_BYTES) problem("name too long.");
    else value.push(name ? { name, url } : { url });
  });
  if (value.length > MAX_PREVIEW_URLS) problems.push(`At most ${MAX_PREVIEW_URLS} URLs.`);
  return { value, problems, bad };
}

/** `env` as `.env` lines that `parseEnv` reads back unchanged. */
export function formatEnv(env: Record<string, string>): string {
  return entries(env)
    .map(([k, v]) => `${k}=${v !== v.trim() || unquote(v) !== v ? `"${v}"` : v}`)
    .join("\n");
}
