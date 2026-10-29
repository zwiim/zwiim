// The Models tab: what a new chat starts with on the host — which agent, in
// which mode, and each agent's model and effort. The host keeps these, so a
// chat started from a phone starts the same way as one started here.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  effortFor,
  type AgentSummary,
  type AgentType,
  type HostSettingKey,
  type HostSettings,
  type SessionMode,
} from "@zwiim/sdk";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { ControlSelect, MODE_LABEL } from "@/panels/OptionsBar";
import type { SettingMeta } from "@/settings/registry";
import { SettingRow } from "@/settings/rows";

/** The agents whose model and effort the host keeps a default for. */
const PER_AGENT: { agent: AgentType; effortLabel: string }[] = [
  { agent: "claude_code", effortLabel: "Effort" },
  { agent: "codex", effortLabel: "Thinking level" },
];

const MODES: SessionMode[] = ["unattended", "normal", "plan"];

/** While the host reads an agent's models, how often to ask again. */
const LEARN_POLL_MS = 2000;

/** What an unset choice shows; the select's value for it is "". */
const UNSET = "";

type Values = HostSettings["values"];

export function ModelsSettings() {
  const controller = useController();
  const online = useOnline();
  const agents = useHost((s) => s.agents);
  const [values, setValues] = useState<Values | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    controller.client.request("settings.get", {}).then(
      (s) => setValues(s.values),
      (e) => setError(errorMessage(e)),
    );
    // What each agent offers comes with the agent list.
    controller.refreshAgents().catch(() => {});
  }, [controller]);

  // An agent the host can run but has not read the models of yet: the host
  // is reading them now (asking for the agent list starts it), so ask again
  // until they arrive.
  const learning = PER_AGENT.some(({ agent }) => {
    const a = agents?.find((x) => x.agent === agent);
    return !!a?.available && !a.options;
  });
  useEffect(() => {
    if (!learning) return;
    const t = setInterval(() => void controller.refreshAgents().catch(() => {}), LEARN_POLL_MS);
    return () => clearInterval(t);
  }, [learning, controller]);

  const update = useCallback(
    (patch: Values) => {
      setError(null);
      controller.client
        .request("settings.update", { values: patch })
        .then((s) => {
          setValues(s.values);
          // New chats read the defaults from the agent list.
          return controller.refreshAgents();
        })
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
  const value = (key: HostSettingKey) => text(values[key]) ?? UNSET;
  const set = (key: HostSettingKey, v: string) => update({ [key]: v === UNSET ? null : v });
  const usable = (agents ?? []).filter((a) => a.available || a.agent === values["defaults.agent"]);

  return (
    <>
      <p className="text-muted-foreground">
        What a new chat starts with on the host, from this computer or any other device. A chat can still change
        them.
      </p>
      <Row id="default-agent" title="Agent">
        <ControlSelect
          label="Default agent"
          value={value("defaults.agent")}
          disabled={!online}
          onChange={(v) => set("defaults.agent", v)}
          options={[
            { value: UNSET, label: "First available" },
            ...usable.map((a) => ({ value: a.agent, label: a.name })),
          ]}
        />
      </Row>
      <Row id="default-mode" title="Mode" description="Plan has the agent propose a plan before it changes anything.">
        <ControlSelect
          label="Default mode"
          value={value("defaults.mode")}
          disabled={!online}
          onChange={(v) => set("defaults.mode", v)}
          options={[
            { value: UNSET, label: `Default (${MODE_LABEL.unattended})` },
            ...MODES.map((m) => ({ value: m, label: MODE_LABEL[m] })),
          ]}
        />
      </Row>
      {PER_AGENT.map(({ agent, effortLabel }) => {
        const summary = agents?.find((a) => a.agent === agent);
        return summary ? (
          <AgentDefaultsRows
            key={agent}
            summary={summary}
            effortLabel={effortLabel}
            model={text(values[`defaults.${agent}.model` as HostSettingKey])}
            effort={text(values[`defaults.${agent}.effort` as HostSettingKey])}
            disabled={!online}
            onChange={(patch) =>
              update(
                Object.fromEntries(Object.entries(patch).map(([k, v]) => [`defaults.${agent}.${k}`, v])) as Values,
              )
            }
          />
        ) : null;
      })}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

/**
 * One agent's model and effort, from what the agent offers on the host. The
 * host reads that before the agent's first chat; until it has, or while the
 * agent cannot run, there is nothing to choose from. Effort is the
 * chosen model's own, so a model without the chosen effort clears it.
 */
function AgentDefaultsRows({
  summary,
  effortLabel,
  model,
  effort,
  disabled,
  onChange,
}: {
  summary: AgentSummary;
  effortLabel: string;
  model: string | null;
  effort: string | null;
  disabled: boolean;
  onChange(patch: { model?: string | null; effort?: string | null }): void;
}) {
  const offered = summary.options?.model;
  if (!summary.options || !offered) {
    return (
      <Row id={`${summary.agent}-model`} title={`${summary.name} model`}>
        <span className="text-muted-foreground">
          {summary.available ? `Reading ${summary.name}'s models…` : `Set up ${summary.name} to choose`}
        </span>
      </Row>
    );
  }
  const efforts = (m: string | null) => effortFor(summary, m ?? offered.current);
  const shownEffort = efforts(model);
  return (
    <>
      <Row id={`${summary.agent}-model`} title={`${summary.name} model`}>
        <ControlSelect
          label={`${summary.name} model`}
          value={model ?? UNSET}
          disabled={disabled}
          onChange={(v) => {
            const next = v === UNSET ? null : v;
            const keeps = !effort || !!efforts(next)?.values.some((o) => o.value === effort);
            onChange(keeps ? { model: next } : { model: next, effort: null });
          }}
          options={[
            { value: UNSET, label: "Agent's default" },
            ...offered.values.map((v) => ({ value: v.value, label: v.label, title: v.description ?? undefined })),
          ]}
        />
      </Row>
      {shownEffort && (
        <Row id={`${summary.agent}-effort`} title={`${summary.name} ${effortLabel.toLowerCase()}`}>
          <ControlSelect
            label={`${summary.name} ${effortLabel.toLowerCase()}`}
            value={effort ?? UNSET}
            disabled={disabled}
            onChange={(v) => onChange({ effort: v === UNSET ? null : v })}
            options={[
              { value: UNSET, label: "Model's default" },
              ...shownEffort.values.map((v) => ({ value: v.value, label: v.label, title: v.description ?? undefined })),
            ]}
          />
        </Row>
      )}
    </>
  );
}

/** A setting the models tab keeps as text; anything else reads as unset. */
function text(v: Values[string]): string | null {
  return typeof v === "string" ? v : null;
}

function Row({
  id,
  title,
  description,
  children,
}: SettingMeta & { children: ReactNode }) {
  return <SettingRow setting={{ id, title, description }}>{children}</SettingRow>;
}
