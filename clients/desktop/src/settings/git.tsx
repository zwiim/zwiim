// The Git tab: how the host names the branches it makes and whether removing
// a worktree takes its branch with it. The host keeps these, so a worktree
// made from a phone is named the same way as one made here. Whether a pull
// request offers Auto-merge is this computer's own.

import { useCallback, useEffect, useState } from "react";
import type { HostSettings } from "@zwiim/sdk";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { autoMergeButton, rememberAutoMergeButton } from "@/lib/prefs";
import { usePref } from "@/lib/use-pref";
import { ControlSelect } from "@/panels/OptionsBar";
import type { SettingMeta } from "@/settings/registry";
import { SettingRow, ToggleRow } from "@/settings/rows";

type Values = HostSettings["values"];

/** The select's value for no prefix; the host keeps it as unset. */
const NONE = "";

/** A city stands in for the name the host picks, to show where the prefix goes. */
const EXAMPLE_CITY = "pune";

export function GitSettings() {
  const controller = useController();
  const online = useOnline();
  const [values, setValues] = useState<Values | null>(null);
  const [custom, setCustom] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("settings.get", {}).then(
      (s) => {
        setValues(s.values);
        setCustom(text(s.values["git.branch_prefix_custom"]));
      },
      (e) => setError(errorMessage(e)),
    );
  }, [controller]);

  const update = useCallback(
    (patch: Values) => {
      setError(null);
      controller.client
        .request("settings.update", { values: patch })
        .then((s) => setValues(s.values))
        .catch((e) => setError(errorMessage(e)));
    },
    [controller],
  );

  if (!values) {
    return error ? (
      <p className="text-destructive" role="alert">
        {error}
      </p>
    ) : null;
  }
  const source = text(values["git.branch_prefix"]);
  const saved = text(values["git.branch_prefix_custom"]);
  const saveCustom = () => {
    const next = custom.trim();
    if (next !== saved) update({ "git.branch_prefix_custom": next === "" ? null : next });
  };
  const example =
    source === "custom" && saved
      ? `${saved.replace(/\/+$/, "")}/${EXAMPLE_CITY}`
      : source === "github"
        ? `your-username/${EXAMPLE_CITY}`
        : EXAMPLE_CITY;

  return (
    <>
      <p className="text-muted-foreground">
        How the host names the branches it makes for new worktrees and continued chats, from this computer or any
        other device. Branches that already exist keep their names.
      </p>
      <SettingRow
        setting={{
          id: "branch-prefix",
          title: "Branch name prefix",
          description: `New branches look like ${example}. The folder keeps the plain name.`,
        }}
      >
        <ControlSelect
          label="Branch name prefix"
          value={source}
          disabled={!online}
          onChange={(v) => update({ "git.branch_prefix": v === NONE ? null : v })}
          options={[
            { value: NONE, label: "None" },
            { value: "github", label: "GitHub username" },
            { value: "custom", label: "Custom" },
          ]}
        />
      </SettingRow>
      {source === "custom" && (
        <SettingRow
          setting={{
            id: "branch-prefix-custom",
            title: "Custom prefix",
            description: "Letters, digits, . _ - and / between parts.",
          }}
        >
          <input
            aria-label="Custom prefix"
            className="bg-input/30 w-40 rounded-md border px-2 py-1 font-mono text-xs"
            placeholder="team/name"
            value={custom}
            disabled={!online}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => setCustom(e.target.value)}
            onBlur={saveCustom}
            onKeyDown={(e) => {
              if (e.key === "Enter") saveCustom();
            }}
          />
        </SettingRow>
      )}
      {source === "github" && (
        <p className="text-muted-foreground">
          Uses the account GitHub is signed in as on the host. Until it is, new branches have no prefix.
        </p>
      )}
      <ToggleRow
        setting={{
          id: "delete-branch-on-remove",
          title: "Delete the branch when removing a worktree",
          description: "Only when its pull request merged or every commit is pushed. Otherwise the branch stays.",
        }}
        testId="delete-branch-on-remove"
        checked={values["git.delete_branch_on_remove"] === true}
        disabled={!online}
        onChange={(on) => update({ "git.delete_branch_on_remove": on })}
      />
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

export function AutoMergeToggle({ setting }: { setting: SettingMeta }) {
  const on = usePref(autoMergeButton);
  return <ToggleRow setting={setting} testId="auto-merge" checked={on} onChange={rememberAutoMergeButton} />;
}

function text(v: Values[string]): string {
  return typeof v === "string" ? v : NONE;
}
