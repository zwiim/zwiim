import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type AgentAuthStatus } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { AgentsSettings } from "@/panels/Agents";
import { FakeTransport, tick } from "./fakes";
import { openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => {
  window.localStorage.clear();
  rememberWelcomed();
});

function bridge(): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u-me", email: "me@example.com" })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

const idle = (agent: string, more: Partial<AgentAuthStatus> = {}): AgentAuthStatus => ({
  agent,
  connected: false,
  method: null,
  saved_key: false,
  sign_in: "idle",
  email: null,
  plan: null,
  ...more,
});

/** A host whose agent sign-in the test changes through its ops. */
function hostWithAgents() {
  const t = new FakeTransport();
  const state: Record<string, AgentAuthStatus> = {
    claude_code: idle("claude_code", {
      connected: true,
      method: "account",
      email: "me@example.com",
      plan: "Claude Max",
    }),
    codex: idle("codex"),
  };
  t.handlers["agent.auth_status"] = () => Object.values(state);
  t.handlers["agent.save_key"] = ({ agent }) =>
    (state[agent] = { ...state[agent]!, saved_key: true, connected: true, method: "api_key" });
  t.handlers["agent.remove_key"] = ({ agent }) => (state[agent] = idle(agent));
  t.handlers["agent.sign_out"] = ({ agent }) => (state[agent] = idle(agent));
  t.handlers["agent.sign_in"] = ({ agent }) => (state[agent] = { ...state[agent]!, sign_in: "running" });
  return { t, state };
}

async function mountWorkbench(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Workbench bridge={bridge()} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

describe("agent sign-in", () => {
  it("shows each agent's sign-in and saves a key that never comes back", async () => {
    const { t } = hostWithAgents();
    const c = await mountWorkbench(t);
    const dialog = openSettings("Agents");
    await tick();

    const status = dialog.getByTestId("agent-status");
    expect(status.textContent).toContain("Connected");
    expect(status.textContent).toContain("Claude subscription");
    expect(status.textContent).toContain("me@example.com · Claude Max");
    // Already signed in with the account: no sign-in to offer.
    const account = dialog.getByTestId("agent-account");
    expect(account.textContent).toContain("Signed in with your Claude subscription.");
    expect(within(account).queryByRole("button", { name: "Sign in" })).toBeNull();

    fireEvent.click(dialog.getByRole("tab", { name: "Codex" }));
    expect(dialog.getByTestId("agent-status").textContent).toContain("Not signed in");
    expect(dialog.queryByRole("button", { name: "Sign out" })).toBeNull();

    const input = dialog.getByLabelText("OpenAI API key") as HTMLInputElement;
    expect(input.type).toBe("password");
    fireEvent.change(input, { target: { value: "sk-test-0123456789" } });
    fireEvent.click(dialog.getByRole("button", { name: "Save key" }));
    await tick();
    expect(t.calls.find((x) => x.op === "agent.save_key")?.params).toEqual({ agent: "codex", key: "sk-test-0123456789" });
    expect(input.value).toBe("");
    expect(dialog.getByTestId("agent-status").textContent).toContain("API key");
    expect(dialog.getByTestId("agent-key").textContent).toContain("Saved on this Mac");
    expect(dialog.getByTestId("settings-agents").textContent).not.toContain("sk-test");

    fireEvent.click(dialog.getByRole("button", { name: "Remove key" }));
    await tick();
    expect(t.calls.some((x) => x.op === "agent.remove_key" && x.params.agent === "codex")).toBe(true);
    expect(dialog.getByTestId("agent-status").textContent).toContain("Not signed in");
    c.dispose();
  });

  it("asks before signing out, then signs the agent out", async () => {
    const { t } = hostWithAgents();
    const c = await mountWorkbench(t);
    const dialog = openSettings("Agents");
    await tick();

    fireEvent.click(dialog.getByRole("button", { name: "Sign out" }));
    expect(t.calls.some((x) => x.op === "agent.sign_out")).toBe(false);
    expect(dialog.getByTestId("settings-agents").textContent).toContain("running sessions stop");
    fireEvent.click(dialog.getByRole("button", { name: "Sign out" }));
    await tick();
    expect(t.calls.find((x) => x.op === "agent.sign_out")?.params).toEqual({ agent: "claude_code" });
    expect(dialog.getByTestId("agent-status").textContent).toContain("Not signed in");
    c.dispose();
  });

  it("starts an account sign-in and follows it until it ends", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const { t, state } = hostWithAgents();
      const c = await mountWorkbench(t);
      const dialog = openSettings("Agents");
      await tick();
      fireEvent.click(dialog.getByRole("tab", { name: "Codex" }));
      fireEvent.click(dialog.getByRole("button", { name: "Sign in" }));
      await tick();
      expect(dialog.getByTestId("agent-account").textContent).toContain("in the browser");

      state.codex = idle("codex", { connected: true, method: "account", email: "me@example.com", plan: "ChatGPT Plus" });
      vi.advanceTimersByTime(2000);
      await tick();
      expect(dialog.getByTestId("agent-status").textContent).toContain("ChatGPT Plus");
      expect(dialog.getByTestId("agent-account").textContent).not.toContain("in the browser");
      expect(dialog.queryByRole("button", { name: "Sign in" })).toBeNull();
      c.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows a remote host's sign-in without a way to change it", async () => {
    const { t } = hostWithAgents();
    const client = new HostClient({ transport: t, random: () => 0.5 });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <AgentsSettings remote />
      </HostProvider>,
    );
    await tick();
    expect(screen.getByTestId("agent-status").textContent).toContain("Connected");
    expect(screen.getByTestId("agent-remote").textContent).toBe("Sign in on the Mac running Zwiim.");
    expect(screen.queryByRole("button", { name: "Sign in" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Sign out" })).toBeNull();
    expect(screen.queryByLabelText("Anthropic API key")).toBeNull();
    expect(within(screen.getByRole("tablist")).getAllByRole("tab")).toHaveLength(2);
    controller.dispose();
  });
});
