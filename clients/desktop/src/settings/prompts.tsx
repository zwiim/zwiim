// The Prompts tab: how the user wants agents to open pull requests and name
// branches, in their own words. The host keeps them and adds them for agents
// however a chat was started, so a chat begun from a phone follows them too.
// Everything else they ask of agents is in the Instructions tab.

import { useCallback, useEffect, useState } from "react";
import { MAX_PROMPT_BYTES, type HostSettingKey, type HostSettings } from "@zwiim/sdk";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { useOpenSettings } from "@/settings/open";

type Values = HostSettings["values"];

interface PromptDef {
  key: HostSettingKey;
  id: string;
  title: string;
  description: string;
  placeholder: string;
}

const PROMPTS: readonly PromptDef[] = [
  {
    key: "prompts.pull_request",
    id: "prompt-pull-request",
    title: "Pull requests",
    description: "When an agent opens a pull request.",
    placeholder: "Open pull requests as drafts. Start the title with the area changed.",
  },
  {
    key: "prompts.branch_name",
    id: "prompt-branch-name",
    title: "Branch names",
    description: "When an agent names the branch of a new worktree.",
    placeholder: "Start with the ticket number, like 123-fix-login.",
  },
];

const encoder = new TextEncoder();

export function PromptsSettings() {
  const controller = useController();
  const online = useOnline();
  const openSettings = useOpenSettings();
  const [values, setValues] = useState<Values | null>(null);
  const [drafts, setDrafts] = useState<Partial<Record<HostSettingKey, string>>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("settings.get", {}).then(
      (s) => {
        setValues(s.values);
        setDrafts(Object.fromEntries(PROMPTS.map((p) => [p.key, text(s.values[p.key])])));
      },
      (e) => setError(errorMessage(e)),
    );
  }, [controller]);

  const save = useCallback(
    (key: HostSettingKey, draft: string) => {
      const next = draft.trim();
      if (next === text(values?.[key])) return;
      setError(null);
      controller.client
        .request("settings.update", { values: { [key]: next === "" ? null : next } })
        .then((s) => setValues(s.values))
        .catch((e) => setError(errorMessage(e)));
    },
    [controller, values],
  );

  if (!values) {
    return error ? (
      <p className="text-destructive" role="alert">
        {error}
      </p>
    ) : null;
  }

  return (
    <>
      <p className="text-muted-foreground">
        How agents on the host open pull requests and name branches, from this computer or any other device. Claude
        Code and Codex read these when a chat starts, so a change reaches the next chat, not one already open. What
        every agent should know about all work, or about resolving merge conflicts, is in{" "}
        {openSettings ? (
          <button type="button" className="text-foreground underline underline-offset-2" onClick={() => openSettings({ tab: "instructions" })}>
            Instructions
          </button>
        ) : (
          "Instructions"
        )}
        .
      </p>
      {PROMPTS.map((p) => {
        const draft = drafts[p.key] ?? "";
        const over = encoder.encode(draft.trim()).length > MAX_PROMPT_BYTES;
        return (
          <section key={p.key} className="flex flex-col gap-1.5 border-b pb-3" data-setting-id={p.id}>
            <label htmlFor={p.id} className="text-ui">
              {p.title}
            </label>
            <span className="text-muted-foreground">{p.description}</span>
            <textarea
              id={p.id}
              className="bg-input/30 placeholder:text-muted-foreground min-h-20 w-full resize-y rounded-md border px-2 py-1.5 leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
              placeholder={p.placeholder}
              value={draft}
              disabled={!online}
              aria-invalid={over || undefined}
              onChange={(e) => setDrafts((d) => ({ ...d, [p.key]: e.target.value }))}
              onBlur={() => {
                if (!over) save(p.key, draft);
              }}
            />
            {over && (
              <p className="text-destructive" role="alert">
                Too long for the host to keep. Shorten it to save.
              </p>
            )}
          </section>
        );
      })}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

function text(v: Values[string] | undefined): string {
  return typeof v === "string" ? v : "";
}
