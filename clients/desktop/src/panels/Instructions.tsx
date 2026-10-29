import { useEffect, useId, useState } from "react";
import {
  hasCapability,
  instructionCounter,
  instructionsErrorText,
  instructionsFit,
  instructionTooLong,
  type InstructionTexts,
  type InstructionsChange,
  type UserInstructions,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";

export function useInstructionsOffered(): boolean {
  return useHost((s) => hasCapability(s.connection.compat, "host.instructions"));
}

/**
 * The user's instructions for their agents: Host-wide, and for the selected
 * workspace's repository, which its worktrees share.
 */
export function InstructionsSettings({ workspaceId }: { workspaceId: string | null }) {
  const controller = useController();
  const workspaces = useHost((s) => s.workspaces);
  const [value, setValue] = useState<UserInstructions | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    controller.client
      .request("host.instructions", {})
      .then((v) => live && setValue(v))
      .catch((e) => live && setLoadError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [controller]);

  const selected = workspaces?.find((w) => w.workspace_id === workspaceId) ?? null;
  const rootId = selected ? (selected.parent_workspace_id ?? selected.workspace_id) : null;
  const root = workspaces?.find((w) => w.workspace_id === rootId && !w.removed) ?? null;

  if (loadError) {
    return (
      <p className="text-destructive" role="alert">
        {loadError}
      </p>
    );
  }
  if (!value) return <p className="text-muted-foreground">Loading…</p>;

  const save = async (change: InstructionsChange) => {
    setValue(await controller.client.request("host.instructions.set", change));
  };
  const none: InstructionTexts = { general: null, resolve_conflicts: null };
  return (
    <>
      <p className="text-muted-foreground">Added to what Zwiim tells your agents.</p>
      <ScopeEditor
        key={`host:${JSON.stringify(value.host)}`}
        title="Every workspace"
        saved={value.host}
        onSave={(texts) => save({ scope: "host", ...texts })}
      />
      {root && (
        <ScopeEditor
          key={`${root.workspace_id}:${JSON.stringify(value.workspaces[root.workspace_id] ?? none)}`}
          title={root.alias ?? root.name}
          note="Its worktrees use these too."
          saved={value.workspaces[root.workspace_id] ?? none}
          onSave={(texts) => save({ scope: "workspace", workspace_id: root.workspace_id, ...texts })}
        />
      )}
    </>
  );
}

type Drafts = { [K in keyof InstructionTexts]: string };

const FIELDS: { key: keyof InstructionTexts; label: string; placeholder: string }[] = [
  { key: "general", label: "Instructions for every session", placeholder: "Run the tests before you finish." },
  { key: "resolve_conflicts", label: "When resolving conflicts", placeholder: "Keep our lockfile; regenerate it after." },
];

function ScopeEditor({
  title,
  note,
  saved,
  onSave,
}: {
  title: string;
  note?: string;
  saved: InstructionTexts;
  onSave(texts: Drafts): Promise<void>;
}) {
  const titleId = useId();
  const initial: Drafts = { general: saved.general ?? "", resolve_conflicts: saved.resolve_conflicts ?? "" };
  const [drafts, setDrafts] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changed = FIELDS.some((f) => drafts[f.key] !== initial[f.key]);
  const fits = instructionsFit(drafts);

  const submit = async () => {
    setSaving(true);
    setError(null);
    try {
      await onSave(drafts);
    } catch (e) {
      setError(instructionsErrorText(e) ?? errorMessage(e));
      setSaving(false);
    }
  };

  return (
    <section className="flex flex-col gap-2 rounded-md border p-3" aria-labelledby={titleId}>
      <span id={titleId} className="font-medium">
        {title}
      </span>
      {note && <p className="text-muted-foreground">{note}</p>}
      {FIELDS.map((f) => {
        const tooLong = instructionTooLong(drafts[f.key]);
        return (
          <div key={f.key} className="flex flex-col gap-1">
            <span className="text-muted-foreground">{f.label}</span>
            <textarea
              className="bg-background min-h-16 rounded-md border px-2 py-1.5 text-xs"
              aria-label={f.label}
              placeholder={f.placeholder}
              value={drafts[f.key]}
              onChange={(e) => setDrafts({ ...drafts, [f.key]: e.target.value })}
              disabled={saving}
            />
            <div className="flex items-baseline gap-2 text-2xs">
              {tooLong && <span className="text-destructive">{tooLong}</span>}
              <span className={`${tooLong ? "text-destructive" : "text-muted-foreground"} ml-auto`}>
                {instructionCounter(drafts[f.key])}
              </span>
            </div>
          </div>
        );
      })}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground flex-1">Applies to new sessions, and to open ones after a few idle minutes.</span>
        <Button size="sm" disabled={!changed || !fits || saving} onClick={() => void submit()}>
          Save
        </Button>
      </div>
    </section>
  );
}
