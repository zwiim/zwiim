// The Browser tab: how the host's browser treats agents and sessions. Where it
// saves downloads, how many tabs a session may hold (its pages' own windows
// included), whether agents may type passwords anywhere or must ask before a
// new site, and whether dev servers open in the session's tab.

import { useCallback, useEffect, useState } from "react";
import { DEFAULT_SESSION_TABS, DEFAULT_TIMELINE_DAYS, DEFAULT_TIMELINE_MB, type HostSettings } from "@zwiim/sdk";
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
/** The recording sizes (MB) and ages (days) offered, beside the defaults. */
const TIMELINE_MBS = [10, 200, 1000];
const TIMELINE_DAYS = [1, 30, 90];

/** `offered` with `kept` among them when it was set elsewhere, in order. */
function withKept(offered: number[], kept: unknown): number[] {
  return typeof kept === "number" && !offered.includes(kept) ? [...offered, kept].sort((a, b) => a - b) : offered;
}

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

  // A value set elsewhere stays shown as chosen.
  const tabs = values["browser.session_tabs"];
  const counts = withKept(TAB_COUNTS, tabs);
  const mb = values["browser.timeline_mb"];
  const days = values["browser.timeline_days"];

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
      <SettingRow
        setting={{
          id: "browser-timeline-mb",
          title: "Recording per session",
          description:
            "Each step an agent takes in its browser is kept with a small picture of the page. The oldest steps go first past this size.",
        }}
      >
        <ControlSelect
          label="Recording per session"
          value={typeof mb === "number" ? String(mb) : DEFAULT}
          disabled={!online}
          onChange={(v) => update({ "browser.timeline_mb": v === DEFAULT ? null : Number(v) })}
          options={[
            { value: DEFAULT, label: `${DEFAULT_TIMELINE_MB} MB (default)` },
            ...withKept(TIMELINE_MBS, mb).map((n) => ({ value: String(n), label: `${n} MB` })),
          ]}
        />
      </SettingRow>
      <SettingRow
        setting={{
          id: "browser-timeline-days",
          title: "Keep recordings for",
          description: "Steps older than this are deleted.",
        }}
      >
        <ControlSelect
          label="Keep recordings for"
          value={typeof days === "number" ? String(days) : DEFAULT}
          disabled={!online}
          onChange={(v) => update({ "browser.timeline_days": v === DEFAULT ? null : Number(v) })}
          options={[
            { value: DEFAULT, label: `${DEFAULT_TIMELINE_DAYS} days (default)` },
            ...withKept(TIMELINE_DAYS, days).map((n) => ({ value: String(n), label: n === 1 ? "1 day" : `${n} days` })),
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
