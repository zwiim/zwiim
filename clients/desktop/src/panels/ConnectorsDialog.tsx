// Connectors: outside services agents reach through the Host. The composer's
// plug opens a dialog with the connectors on the left and the one picked on
// the right: where it is allowed, its tools (Off, Ask or Allow each) and the
// limit on how much of an agent's context they take.

import { useCallback, useEffect, useMemo, useState } from "react";
import { Loader2, Plug, Plus, Search, X } from "lucide-react";
import {
  isClientError,
  isHostOutdated,
  type Connector,
  type ConnectorGrant,
  type ConnectorTool,
  type HostController,
} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import {
  allowedHere,
  changed,
  count,
  guessName,
  isLocalUrl,
  limitSteps,
  limitThatFits,
  rootOf,
  scopeOf,
  shownGrant,
  statusLine,
  toolChoice,
  toolGroup,
  type Scope,
  type Tone,
  type ToolChoice,
  type ToolGroup,
} from "@/lib/connectors";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import { relativeTime } from "./GitPanel";

/** The connectors and each one's grants, as the Host last reported them. */
interface Loaded {
  connectors: Connector[];
  grants: Record<string, ConnectorGrant[]>;
}

async function load(controller: HostController): Promise<Loaded> {
  const { connectors } = await controller.listConnectors();
  const shown = connectors.filter((c) => c.status !== "revoked");
  const pairs = await Promise.all(
    shown.map(async (c) => [c.connector_id, (await controller.listConnectorGrants(c.connector_id)).grants] as const),
  );
  return { connectors: shown, grants: Object.fromEntries(pairs) };
}

/** "Linear", or "Linear (Work)" when the user named it. */
const display = (c: Connector) => (c.label === "Default" ? c.name : `${c.name} (${c.label})`);

const TONE_TEXT: Record<Tone, string> = {
  ok: "text-done",
  brand: "text-brand",
  bad: "text-destructive",
  faint: "text-faint",
};
const TONE_DOT: Record<Tone, string> = {
  ok: "bg-done",
  brand: "bg-brand",
  bad: "bg-destructive",
  faint: "bg-faint",
};

/**
 * The composer's plug: how many connectors agents here can use, and the
 * dialog. Hidden for a Host from before connectors.
 */
export function ConnectorsButton({ workspaceId }: { workspaceId: string }) {
  const controller = useController();
  const online = useOnline();
  const root = useHost((s) => rootOf(s.workspaces, workspaceId));
  const [data, setData] = useState<Loaded | null>(null);
  const [outdated, setOutdated] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    // Counted again once the dialog closes: it may have changed them.
    if (!online || open) return;
    let live = true;
    load(controller).then(
      (d) => live && setData(d),
      (e) => live && isHostOutdated(e) && setOutdated(true),
    );
    return () => {
      live = false;
    };
  }, [controller, online, open]);

  if (outdated) return null;
  const here =
    data?.connectors.filter((c) => c.status === "active" && allowedHere(data.grants[c.connector_id] ?? [], root))
      .length ?? 0;
  return (
    <>
      <Button
        size="icon-sm"
        variant="ghost"
        className="relative"
        title="Connectors"
        aria-label={here ? `Connectors, ${here} on here` : "Connectors"}
        onClick={() => setOpen(true)}
      >
        <Plug />
        {here > 0 && (
          <span className="bg-brand text-brand-foreground absolute -top-0.5 -right-0.5 grid h-3.5 min-w-3.5 place-items-center rounded-full px-0.5 text-[10px] leading-none font-medium">
            {here}
          </span>
        )}
      </Button>
      {open && <ConnectorsDialog workspaceId={workspaceId} onClose={() => setOpen(false)} />}
    </>
  );
}

export function ConnectorsDialog({ workspaceId, onClose }: { workspaceId: string; onClose(): void }) {
  const controller = useController();
  const workspaces = useHost((s) => s.workspaces);
  const root = rootOf(workspaces, workspaceId);
  const rootName = workspaces?.find((w) => w.workspace_id === root)?.name ?? "this workspace";
  const [data, setData] = useState<Loaded | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  const reload = useCallback(async () => {
    try {
      const d = await load(controller);
      setData(d);
      setFailed(null);
      return d;
    } catch (e) {
      setFailed(errorMessage(e));
      return null;
    }
  }, [controller]);

  useEffect(() => {
    void reload().then((d) => {
      if (!d) return;
      const first = d.connectors.find((c) => allowedHere(d.grants[c.connector_id] ?? [], root)) ?? d.connectors[0];
      setSelected((cur) => cur ?? first?.connector_id ?? "add");
    });
  }, [reload, root]);

  const update = useCallback((c: Connector) => {
    setData((d) => d && { ...d, connectors: d.connectors.map((x) => (x.connector_id === c.connector_id ? c : x)) });
  }, []);
  const updateGrants = useCallback((id: string, grants: ConnectorGrant[]) => {
    setData((d) => d && { ...d, grants: { ...d.grants, [id]: grants } });
  }, []);

  const q = query.trim().toLowerCase();
  const matching = (data?.connectors ?? []).filter(
    (c) => !q || [c.name, c.label, c.url].some((s) => s.toLowerCase().includes(q)),
  );
  const here = matching.filter((c) => allowedHere(data?.grants[c.connector_id] ?? [], root));
  const elsewhere = matching.filter((c) => !here.includes(c));
  const current = data?.connectors.find((c) => c.connector_id === selected) ?? null;
  const fileKeys = data?.connectors.some((c) => c.credentials === "file") ?? false;

  return (
    <Dialog
      onClose={onClose}
      aria-label="Connectors"
      className="h-[min(660px,calc(100vh-48px))] max-w-[900px] flex-row overflow-hidden"
    >
      <aside className="border-border flex w-68 shrink-0 flex-col border-r bg-muted">
        <div className="flex h-16 shrink-0 items-center justify-between pr-3 pl-3.5">
          <h2 className="text-base font-medium">Connectors</h2>
          <Button
            size="icon-sm"
            variant="secondary"
            title="Add connector"
            aria-label="Add connector"
            onClick={() => setSelected("add")}
          >
            <Plus />
          </Button>
        </div>
        <label className="bg-background ring-border text-faint mx-3 mb-2 flex h-8 items-center gap-2 rounded-lg px-2.5 ring-1 ring-inset">
          <Search className="size-3.5 shrink-0" />
          <input
            className="text-foreground placeholder:text-faint text-ui min-w-0 flex-1 bg-transparent outline-none"
            placeholder="Search"
            aria-label="Search connectors"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
          {!data && !failed && <p className="text-faint px-2 py-3 text-xs">Loading…</p>}
          {failed && <p className="text-destructive px-2 py-3 text-xs">{failed}</p>}
          {data && data.connectors.length === 0 && (
            <p className="text-muted-foreground px-2 py-3 text-xs">No connectors yet. Add one with +.</p>
          )}
          {here.length > 0 && (
            <ListSection title={`Allowed in ${rootName} · ${here.length}`}>
              {here.map((c) => (
                <ListItem
                  key={c.connector_id}
                  connector={c}
                  line={usage(c)}
                  dot
                  selected={selected}
                  onSelect={setSelected}
                />
              ))}
            </ListSection>
          )}
          {elsewhere.length > 0 && (
            <ListSection title={`Not allowed here · ${elsewhere.length}`}>
              {elsewhere.map((c) => (
                <ListItem
                  key={c.connector_id}
                  connector={c}
                  line={data?.grants[c.connector_id]?.length ? "other workspaces" : "not allowed yet"}
                  dot={c.status !== "active"}
                  selected={selected}
                  onSelect={setSelected}
                />
              ))}
            </ListSection>
          )}
        </div>
        <p className="border-border text-faint border-t px-3.5 py-3 font-mono text-2xs">
          {fileKeys ? "Some keys are kept in a file on the Host" : "Keys stay in the system keychain"}
        </p>
      </aside>
      <section className="flex min-w-0 flex-1 flex-col">
        {selected === "add" ? (
          <AddConnector
            root={root}
            rootName={rootName}
            onClose={onClose}
            onCancel={() => setSelected(data?.connectors[0]?.connector_id ?? null)}
            onAdded={async (c) => {
              await reload();
              setSelected(c.connector_id);
            }}
          />
        ) : current ? (
          <ConnectorDetail
            key={current.connector_id}
            connector={current}
            grants={data?.grants[current.connector_id] ?? []}
            root={root}
            rootName={rootName}
            onConnector={update}
            onGrants={(g) => updateGrants(current.connector_id, g)}
            onRemoved={async () => {
              const d = await reload();
              setSelected(d?.connectors[0]?.connector_id ?? "add");
            }}
            onClose={onClose}
          />
        ) : (
          <div className="flex justify-end p-3">
            <CloseButton onClose={onClose} />
          </div>
        )}
      </section>
    </Dialog>
  );
}

function usage(c: Connector): string | null {
  if (c.status !== "active") return null;
  return `${count(c.tools_used)} / ${count(c.tool_budget)}`;
}

function ListSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="pt-2">
      <h3 className="text-muted-foreground px-2 pt-1.5 pb-1 text-[10px] font-semibold tracking-wider uppercase">{title}</h3>
      {children}
    </div>
  );
}

function ListItem({
  connector,
  line,
  dot,
  selected,
  onSelect,
}: {
  connector: Connector;
  line: string | null;
  /** Shown for a problem, or a connector agents here can use. */
  dot: boolean;
  selected: string | null;
  onSelect(id: string): void;
}) {
  const status = statusLine(connector);
  const problem = connector.status !== "active";
  return (
    <button
      type="button"
      data-testid={`connector-${connector.connector_id}`}
      aria-current={selected === connector.connector_id ? "true" : undefined}
      className={cn(
        "ease-std flex w-full items-center gap-2.5 rounded-lg px-2 py-2 text-left transition-colors duration-120",
        selected === connector.connector_id ? "bg-accent" : "hover:bg-accent/60",
      )}
      onClick={() => onSelect(connector.connector_id)}
    >
      <Logo name={connector.name} />
      <span className="min-w-0 flex-1">
        <span className="text-ui block truncate">{display(connector)}</span>
        <span className={cn("block truncate font-mono text-2xs", problem ? TONE_TEXT[status.tone] : "text-faint")}>
          {problem ? status.text : line}
        </span>
      </span>
      {dot && (
        <span className={cn("size-1.5 shrink-0 rounded-full", TONE_DOT[status.tone])} aria-hidden="true" />
      )}
    </button>
  );
}

function Logo({ name, large }: { name: string; large?: boolean }) {
  return (
    <span
      className={cn(
        "bg-secondary text-foreground grid shrink-0 place-items-center font-semibold",
        large ? "size-8 rounded-lg text-sm" : "size-5 rounded text-2xs",
      )}
      aria-hidden="true"
    >
      {name.charAt(0).toUpperCase()}
    </span>
  );
}

function CloseButton({ onClose }: { onClose(): void }) {
  return (
    <Button size="icon-sm" variant="ghost" title="Close" aria-label="Close" onClick={onClose}>
      <X />
    </Button>
  );
}

function Caption({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-muted-foreground flex h-7 items-center gap-2 text-[10px] font-semibold tracking-wider uppercase">
      {children}
    </div>
  );
}

const GROUPS: { group: ToolGroup; title: string }[] = [
  { group: "read", title: "Read" },
  { group: "write", title: "Write" },
  { group: "destructive", title: "Destructive" },
];

/** A tool that didn't fit: turning it on waits for a higher limit. */
interface Over {
  tool: string;
  choice: ToolChoice;
}

function ConnectorDetail({
  connector,
  grants,
  root,
  rootName,
  onConnector,
  onGrants,
  onRemoved,
  onClose,
}: {
  connector: Connector;
  grants: ConnectorGrant[];
  root: string;
  rootName: string;
  onConnector(c: Connector): void;
  onGrants(grants: ConnectorGrant[]): void;
  onRemoved(): Promise<void>;
  onClose(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const id = connector.connector_id;
  const [tools, setTools] = useState<ConnectorTool[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [over, setOver] = useState<Over | null>(null);
  const [opened, setOpened] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [added, setAdded] = useState<string[]>([]);
  const [lastCall, setLastCall] = useState<string | null | undefined>(undefined);
  const [confirming, setConfirming] = useState(false);
  const [key, setKey] = useState("");
  const grant = shownGrant(grants, root);
  const scope = scopeOf(grant);

  useEffect(() => {
    let live = true;
    controller.connectorTools(id).then(
      (r) => live && setTools(r.tools),
      (e) => live && setError(errorMessage(e)),
    );
    controller.listConnectorExecutions({ connector_id: id, limit: 1 }).then(
      (r) => live && setLastCall(r.executions[0]?.received_at ?? null),
      () => live && setLastCall(null),
    );
    return () => {
      live = false;
    };
  }, [controller, id]);

  async function run(what: string, fn: () => Promise<void>) {
    if (busy !== null) return;
    setBusy(what);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const putGrant = (g: ConnectorGrant) => onGrants([...grants.filter((x) => x.grant_id !== g.grant_id), g]);

  async function setEnabled(tool: string, enabled: boolean) {
    const r = await controller.setConnectorToolEnabled(id, tool, enabled);
    onConnector(r.connector);
    setTools(r.tools);
  }

  /** Off is the tool's switch; Ask and Allow turn it on and set its row here. */
  async function apply(tool: ConnectorTool, choice: ToolChoice) {
    if (choice === "off") {
      if (tool.enabled) await setEnabled(tool.name, false);
      return;
    }
    if (!tool.enabled) {
      try {
        await setEnabled(tool.name, true);
      } catch (e) {
        if (!isClientError(e, "over_budget")) throw e;
        setOver({ tool: tool.name, choice });
        return;
      }
    }
    const row = grant?.tools.find((r) => r.name === tool.name);
    if (grant && (row?.policy !== choice || row.changed)) {
      putGrant((await controller.setConnectorToolPolicy(grant.grant_id, tool.name, choice)).grant);
    }
  }

  const choose = (tool: ConnectorTool, choice: ToolChoice) =>
    run(tool.name, async () => {
      setOver(null);
      await apply(tool, choice);
    });

  const raise = (tool: ConnectorTool, choice: ToolChoice, limit: number) =>
    run(tool.name, async () => {
      onConnector((await controller.setConnectorToolBudget(id, limit)).connector);
      setOver(null);
      await apply(tool, choice);
    });

  const setLimit = (limit: number) =>
    run("limit", async () => {
      onConnector((await controller.setConnectorToolBudget(id, limit)).connector);
    });

  /** Moving keeps each tool's row; one given for an earlier version asks. */
  const moveTo = (next: Scope) =>
    run("scope", async () => {
      if (next === scope) return;
      if (next === "nowhere") {
        if (grant) await controller.removeConnectorGrant(grant.grant_id);
        onGrants(grants.filter((g) => g.grant_id !== grant?.grant_id));
        return;
      }
      let made = (
        await controller.setConnectorGrant({
          connector_id: id,
          ...(next === "workspace" ? { workspace_id: root } : {}),
          default_policy: grant?.default_policy ?? "ask",
          new_tools: grant?.new_tools ?? "ask",
        })
      ).grant;
      const listed = new Set(tools?.map((t) => t.name));
      for (const row of grant?.tools ?? []) {
        if (!listed.has(row.name)) continue;
        const policy = row.changed ? "ask" : row.policy;
        made = (await controller.setConnectorToolPolicy(made.grant_id, row.name, policy)).grant;
      }
      if (grant) await controller.removeConnectorGrant(grant.grant_id);
      onGrants([...grants.filter((g) => g.grant_id !== grant?.grant_id && g.grant_id !== made.grant_id), made]);
    });

  const allowRead = () =>
    run("read", async () => {
      if (!grant) return;
      let latest = grant;
      for (const t of tools ?? []) {
        if (toolGroup(t) !== "read" || toolChoice(t, latest) !== "ask") continue;
        latest = (await controller.setConnectorToolPolicy(grant.grant_id, t.name, "allow")).grant;
      }
      putGrant(latest);
    });

  const checkAgain = () =>
    run("check", async () => {
      const r = await controller.validateConnector(id);
      onConnector(r.connector);
      setAdded(r.changes.added);
      setTools((await controller.connectorTools(id)).tools);
    });

  const saveKey = () =>
    run("key", async () => {
      const r = await controller.setConnectorSecret(id, key);
      setKey("");
      onConnector(r.connector);
      if (r.validation_error) setError(r.validation_error.message);
      setTools((await controller.connectorTools(id)).tools);
    });

  const disconnect = () =>
    run("disconnect", async () => {
      await controller.revokeConnector(id);
      await onRemoved();
    });

  const status = statusLine(connector);
  const q = query.trim().toLowerCase();
  const shown = (tools ?? []).filter(
    (t) => !q || [t.name, t.title ?? "", t.description ?? ""].some((s) => s.toLowerCase().includes(q)),
  );
  const host = useMemo(() => {
    try {
      return new URL(connector.url).host;
    } catch {
      return connector.url;
    }
  }, [connector.url]);
  // Not while a change runs: disabling the control just clicked would drop
  // focus out of the dialog. `run` ignores clicks until the Host answers.
  const disabled = !online;
  const steps = limitSteps();
  if (!steps.includes(connector.tool_budget)) steps.push(connector.tool_budget);
  steps.sort((a, b) => a - b);

  return (
    <>
      <header className="flex h-16 shrink-0 items-center gap-3 pr-3 pl-5">
        <Logo name={connector.name} large />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-base font-medium">{display(connector)}</h3>
          <div className="text-muted-foreground truncate font-mono text-2xs">
            {host} · {connector.tool_count} tools · <span className={TONE_TEXT[status.tone]}>{status.text}</span>
          </div>
        </div>
        <Button size="xs" variant="ghost" disabled={disabled} onClick={checkAgain}>
          {busy === "check" && <Loader2 className="animate-spin" />}
          Check again
        </Button>
        <CloseButton onClose={onClose} />
      </header>
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 pb-5">
        {connector.status === "error" && connector.error && (
          <p className="text-destructive text-xs">{connector.error}</p>
        )}
        {error && (
          <p className="text-destructive text-xs" role="alert">
            {error}
          </p>
        )}
        <div>
          <Caption>Access</Caption>
          <div className="ring-border divide-border divide-y rounded-[10px] bg-muted ring-1 ring-inset">
            <Row title="Allowed in" detail={`Agents in these workspaces can use ${connector.name}`}>
              <div className="bg-background flex gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label="Allowed in">
                {(
                  [
                    ["nowhere", "Nowhere"],
                    ["workspace", rootName],
                    ["all", "All workspaces"],
                  ] as const
                ).map(([value, label]) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={scope === value}
                    disabled={disabled}
                    className={cn(
                      "rounded-md px-2.5 py-1 text-xs font-medium transition-colors",
                      scope === value ? "bg-secondary text-foreground" : "text-muted-foreground enabled:hover:text-foreground",
                    )}
                    onClick={() => moveTo(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </Row>
            {(connector.credentials !== null || connector.status === "needs_reauth") && (
              <Row title="Key" detail="Kept on the Host; agents never see it">
                <input
                  type="password"
                  aria-label="New key"
                  placeholder="Paste a new key"
                  className="bg-background ring-input text-ui h-7 w-48 rounded-md px-2 font-mono ring-1 outline-none ring-inset"
                  value={key}
                  onChange={(e) => setKey(e.target.value)}
                />
                <Button size="xs" variant="outline" disabled={disabled || !key.trim()} onClick={saveKey}>
                  Save
                </Button>
              </Row>
            )}
          </div>
        </div>
        <div>
          <Caption>
            Tools · {connector.tool_count}
            <span className="ml-auto flex items-center gap-2 font-mono text-[11.5px] font-normal tracking-normal normal-case">
              <span
                className="bg-muted h-1 w-28 overflow-hidden rounded-full"
                role="meter"
                aria-label="Tool limit used"
                aria-valuemin={0}
                aria-valuemax={connector.tool_budget}
                aria-valuenow={connector.tools_used}
              >
                <span
                  className="bg-brand block h-full"
                  style={{ width: `${Math.min(100, (connector.tools_used / connector.tool_budget) * 100)}%` }}
                />
              </span>
              {count(connector.tools_used)} / {count(connector.tool_budget)}
              <select
                aria-label="Tool limit"
                className="bg-background ring-border text-foreground h-6 rounded-md px-1.5 font-sans text-xs ring-1 ring-inset"
                value={connector.tool_budget}
                disabled={disabled}
                onChange={(e) => setLimit(Number(e.target.value))}
              >
                {steps.map((n) => (
                  <option key={n} value={n} disabled={n < connector.tools_used}>
                    Limit {count(n)}
                  </option>
                ))}
              </select>
            </span>
          </Caption>
          <div className="ring-border rounded-[10px] bg-muted ring-1 ring-inset">
            <div className="flex items-center gap-2.5 px-3.5 py-2">
              <label className="bg-background ring-border text-faint flex h-7 flex-1 items-center gap-1.5 rounded-md px-2 ring-1 ring-inset">
                <Search className="size-3.5 shrink-0" />
                <input
                  className="text-foreground placeholder:text-faint min-w-0 flex-1 bg-transparent text-xs outline-none"
                  placeholder={`Search ${connector.tool_count} tools`}
                  aria-label="Search tools"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                />
              </label>
              {/* Tools that don't say they only read count as writes: then there is nothing to allow. */}
              {tools?.some((t) => toolGroup(t) === "read") && (
                <Button
                  size="xs"
                  variant="ghost"
                  className="text-brand"
                  disabled={disabled || !grant}
                  title={grant ? "Allow every read tool that is on" : "Allow it somewhere first"}
                  onClick={allowRead}
                >
                  Allow all read
                </Button>
              )}
            </div>
            {!tools && !error && <p className="text-faint border-border border-t px-3.5 py-3 text-xs">Loading tools…</p>}
            {GROUPS.map(({ group, title }) => {
              const inGroup = shown.filter((t) => toolGroup(t) === group);
              if (inGroup.length === 0) return null;
              return (
                <div key={group}>
                  <div className="border-border text-faint flex h-7 items-center border-t bg-white/[.015] px-3.5 text-[10px] font-semibold tracking-wider uppercase">
                    {title} · {inGroup.length}
                  </div>
                  {inGroup.map((t) => (
                    <ToolRow
                      key={t.name}
                      tool={t}
                      choice={toolChoice(t, grant)}
                      changed={changed(t, grant)}
                      isNew={added.includes(t.name) && !t.enabled}
                      canAllow={grant !== null}
                      disabled={disabled}
                      busy={busy === t.name}
                      opened={opened === t.name}
                      onOpen={() => setOpened(opened === t.name ? null : t.name)}
                      onChoose={(c) => choose(t, c)}
                      over={
                        over?.tool === t.name
                          ? {
                              left: Math.max(0, connector.tool_budget - connector.tools_used),
                              budget: connector.tool_budget,
                              limit: limitThatFits(t.estimate, connector.tools_used),
                              onRaise: (limit) => raise(t, over.choice, limit),
                              onCancel: () => setOver(null),
                            }
                          : null
                      }
                    />
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      </div>
      <footer className="border-border flex shrink-0 items-center gap-2 border-t px-5 py-2.5">
        {confirming ? (
          <>
            <span className="text-ui">Disconnect {connector.name}? Agents lose it and its key is deleted.</span>
            <Button size="xs" variant="destructive" disabled={disabled} onClick={disconnect}>
              Disconnect
            </Button>
            <Button size="xs" variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button
            size="xs"
            variant="ghost"
            className="text-destructive -ml-2"
            disabled={disabled}
            onClick={() => setConfirming(true)}
          >
            Disconnect
          </Button>
        )}
        {!confirming && lastCall !== undefined && (
          <span className="text-faint ml-auto text-xs">
            {lastCall ? `Last call ${relativeTime(lastCall)}` : "No calls yet"}
          </span>
        )}
      </footer>
    </>
  );
}

function Row({ title, detail, children }: { title: string; detail: string; children: React.ReactNode }) {
  return (
    <div className="flex min-h-[50px] items-center gap-3 px-3.5 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="text-ui">{title}</div>
        <div className="text-muted-foreground text-xs">{detail}</div>
      </div>
      {children}
    </div>
  );
}

const CHOICES: { choice: ToolChoice; label: string; on: string }[] = [
  { choice: "off", label: "Off", on: "bg-secondary text-muted-foreground" },
  { choice: "ask", label: "Ask", on: "bg-secondary text-foreground" },
  { choice: "allow", label: "Allow", on: "bg-brand-soft text-brand" },
];

function ToolRow({
  tool,
  choice,
  changed,
  isNew,
  canAllow,
  disabled,
  busy,
  opened,
  onOpen,
  onChoose,
  over,
}: {
  tool: ConnectorTool;
  choice: ToolChoice;
  changed: boolean;
  isNew: boolean;
  canAllow: boolean;
  disabled: boolean;
  busy: boolean;
  opened: boolean;
  onOpen(): void;
  onChoose(choice: ToolChoice): void;
  over: {
    left: number;
    budget: number;
    limit: number | null;
    onRaise(limit: number): void;
    onCancel(): void;
  } | null;
}) {
  return (
    <div data-testid={`tool-${tool.name}`} className="border-border border-t">
      <div className="flex items-center gap-2.5 px-3.5 py-2">
        <button
          type="button"
          className={cn(
            "min-w-0 flex-1 truncate text-left font-mono text-xs",
            choice === "off" && "text-muted-foreground",
          )}
          aria-expanded={opened}
          onClick={onOpen}
        >
          {tool.name}
          {isNew && <span className="text-brand ml-2 font-sans text-[11.5px]">new · off</span>}
          {changed && <span className="text-brand ml-2 font-sans text-[11.5px]">changed · asks again</span>}
        </button>
        {busy && <Loader2 className="text-faint size-3 animate-spin" />}
        <span className="text-faint w-12 text-right font-mono text-2xs" title="About how much of an agent's context it takes">
          ~{count(tool.estimate)}
        </span>
        <div className="bg-background flex gap-px rounded-md p-0.5" role="radiogroup" aria-label={`${tool.name} access`}>
          {CHOICES.map(({ choice: c, label, on }) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={choice === c}
              disabled={disabled || (c === "allow" && !canAllow)}
              title={c === "allow" && !canAllow ? "Allow it somewhere first" : undefined}
              className={cn(
                "rounded px-1.5 py-0.5 text-2xs font-medium transition-colors disabled:cursor-not-allowed",
                choice === c ? on : "text-faint enabled:hover:text-foreground",
              )}
              onClick={() => choice !== c && onChoose(c)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>
      {opened && (
        <p className="text-muted-foreground px-3.5 pb-2.5 text-xs">{tool.description ?? "No description."}</p>
      )}
      {over && (
        <div className="border-border bg-brand-soft flex items-center gap-2.5 border-t px-3.5 py-2.5 text-xs" role="alert">
          <span className="flex-1">
            <span className="font-medium">
              {tool.name} needs ~{count(tool.estimate)}.
            </span>{" "}
            {count(over.left)} left of {count(over.budget)}.
            {over.limit === null && " Turn other tools off to make room."}
          </span>
          {over.limit !== null && (
            <Button size="xs" variant="brand" onClick={() => over.onRaise(over.limit!)}>
              Raise limit to {count(over.limit)}
            </Button>
          )}
          <Button size="xs" variant="ghost" onClick={over.onCancel}>
            Cancel
          </Button>
        </div>
      )}
    </div>
  );
}

function AddConnector({
  root,
  rootName,
  onAdded,
  onCancel,
  onClose,
}: {
  root: string;
  rootName: string;
  onAdded(c: Connector): Promise<void>;
  onCancel(): void;
  onClose(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const [url, setUrl] = useState("");
  const [name, setName] = useState<string | null>(null);
  const [label, setLabel] = useState("Default");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const shownName = name ?? guessName(url);
  const ready = online && !busy && /^https?:\/\//.test(url.trim()) && shownName.trim() && label.trim();

  async function add() {
    setBusy(true);
    setError(null);
    try {
      const secret = key.trim();
      const r = await controller.createConnector({
        name: shownName.trim(),
        url: url.trim(),
        label: label.trim(),
        network: isLocalUrl(url.trim()) ? "local" : "public",
        auth: secret ? { kind: "bearer" } : { kind: "none" },
        ...(secret ? { secret } : {}),
      });
      await controller.setConnectorGrant({
        connector_id: r.connector.connector_id,
        workspace_id: root,
        default_policy: "ask",
        new_tools: "ask",
      });
      await onAdded(r.connector);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <>
      <header className="flex h-16 shrink-0 items-center gap-3 pr-3 pl-5">
        <div className="min-w-0 flex-1">
          <h3 className="text-base font-medium">Add a connector</h3>
          <div className="text-muted-foreground text-xs">An MCP server your agents reach through this Host</div>
        </div>
        <CloseButton onClose={onClose} />
      </header>
      <form
        className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 pb-5"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) void add();
        }}
      >
        <Field label="Server URL">
          <input
            autoFocus
            aria-label="Server URL"
            placeholder="https://mcp.example.com/mcp"
            className={INPUT}
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
        </Field>
        <div className="flex gap-3">
          <Field label="Name">
            <input aria-label="Name" className={INPUT} value={shownName} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Label">
            <input aria-label="Label" className={INPUT} value={label} onChange={(e) => setLabel(e.target.value)} />
          </Field>
        </div>
        <Field label="API key" hint="Saved in the system keychain on the Host. Agents never see it. Leave empty for a server without a key.">
          <input
            type="password"
            aria-label="API key"
            className={INPUT}
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
        </Field>
        {error && (
          <p className="text-destructive text-xs" role="alert">
            {error}
          </p>
        )}
        <button type="submit" hidden />
      </form>
      <footer className="border-border flex shrink-0 items-center gap-2 border-t px-5 py-2.5">
        <span className="text-faint text-xs">Allowed in {rootName} when added</span>
        <Button size="sm" variant="ghost" className="ml-auto" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" variant="brand" disabled={!ready} onClick={() => void add()}>
          {busy && <Loader2 className="animate-spin" />}
          Add {shownName.trim() || "connector"}
        </Button>
      </footer>
    </>
  );
}

const INPUT =
  "bg-background ring-input focus:ring-brand text-ui h-9 w-full rounded-lg px-2.5 font-mono ring-1 outline-none ring-inset";

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="min-w-0 flex-1 space-y-1.5">
      <div className="text-muted-foreground text-xs">{label}</div>
      {children}
      {hint && <p className="text-faint text-xs">{hint}</p>}
    </div>
  );
}
