// The Browser tab: how the host's browser treats agents and sessions. Where it
// saves downloads, how many tabs a session may hold (its pages' own windows
// included), whether agents may type passwords anywhere or must ask before a
// new site, and whether dev servers open in the session's tab.

import { useCallback, useEffect, useState } from "react";
import { DEFAULT_SESSION_TABS, type HostSettings } from "@zwiim/sdk";
import type { NativeBridge } from "@/lib/bridge";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { ControlSelect } from "@/panels/OptionsBar";
import { BrowserDownloadsSetting } from "@/settings/browser-downloads";
import { SettingRow, ToggleRow } from "@/settings/rows";

type Values = HostSettings["values"];

/** The select's value for the host's default; the host keeps it as unset. */
const DEFAULT = "";

/** The tab counts offered, beside the default. */
const TAB_COUNTS = [5, 10, 30];

export function BrowserSettings({ bridge, remote }: { bridge: NativeBridge; remote: boolean }) {
  const controller = useController();
  const online = useOnline();
  const [values, setValues] = useState<Values | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("settings.get", {}).then(
      (s) => setValues(s.values),
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

  const tabs = values["browser.session_tabs"];
  // A count set elsewhere stays shown as chosen.
  const counts = typeof tabs === "number" && !TAB_COUNTS.includes(tabs) ? [...TAB_COUNTS, tabs].sort((a, b) => a - b) : TAB_COUNTS;

  return (
    <>
      <p className="text-muted-foreground">
        The browser runs on the host, for every device. Agents use it through their browser tools.
      </p>
      <SettingRow
        setting={{
          id: "browser-session-tabs",
          title: "Tabs per session",
          description:
            "The most tabs one session holds, those its pages open by themselves included. A page that tries to open more gets no new tab.",
        }}
      >
        <ControlSelect
          label="Tabs per session"
          value={typeof tabs === "number" ? String(tabs) : DEFAULT}
          disabled={!online}
          onChange={(v) => update({ "browser.session_tabs": v === DEFAULT ? null : Number(v) })}
          options={[
            { value: DEFAULT, label: `${DEFAULT_SESSION_TABS} (default)` },
            ...counts.map((n) => ({ value: String(n), label: String(n) })),
          ]}
        />
      </SettingRow>
      {!remote && <BrowserDownloadsSetting bridge={bridge} />}
      <ToggleRow
        setting={{
          id: "browser-agent-passwords",
          title: "Let agents type passwords on any site",
          description:
            "Off, agents type passwords only on this computer's dev servers (localhost), for test logins. On, they can type them into any site's password field.",
        }}
        testId="browser-agent-passwords"
        checked={values["browser.agent_passwords"] === true}
        disabled={!online}
        onChange={(on) => update({ "browser.agent_passwords": on ? true : null })}
      />
      <ToggleRow
        setting={{
          id: "browser-agent-ask-sites",
          title: "Ask before agents open new sites",
          description:
            "Off, agents open any site, except those a session's site list leaves out. On, the first time an agent opens a site in a session, the page waits until you allow it.",
        }}
        testId="browser-agent-ask-sites"
        checked={values["browser.agent_ask_sites"] === true}
        disabled={!online}
        onChange={(on) => update({ "browser.agent_ask_sites": on ? true : null })}
      />
      <ToggleRow
        setting={{
          id: "browser-open-dev-servers",
          title: "Open dev servers in the session's tab",
          description:
            "When a session starts a dev server in its worktree, its browser tab shows it. A tab on another site or another running server is left alone.",
        }}
        testId="browser-open-dev-servers"
        checked={values["browser.open_dev_servers"] !== false}
        disabled={!online}
        onChange={(on) => update({ "browser.open_dev_servers": on ? null : false })}
      />
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
