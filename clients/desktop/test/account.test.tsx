import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { Account, NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { welcomed } from "@/lib/prefs";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openMenu, openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => window.localStorage.clear());

const me: Account = { user_id: "u-me", email: "me@example.com" };

function bridge(account: Account | null, available = true): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available, account })),
    signIn: vi.fn(async () => me),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

async function mount(t: FakeTransport, b: NativeBridge) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Workbench bridge={b} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

const claims = (t: FakeTransport) => t.calls.filter((c) => c.op === "host.claim").map((c) => c.params);

describe("account", () => {
  it("offers Google or local on first run; local needs no account and is remembered", async () => {
    const t = new FakeTransport();
    const c = await mount(t, bridge(null));
    expect(screen.getByText("Continue with Google")).toBeTruthy();
    fireEvent.click(screen.getByText("Continue locally"));
    expect(screen.queryByTestId("welcome")).toBeNull();
    expect(welcomed()).toBe(true);
    expect(screen.getByText("Local mode")).toBeTruthy();
    expect(claims(t)).toEqual([]);
    c.dispose();
  });

  it("hides sign-in in a build without it", async () => {
    const t = new FakeTransport();
    const c = await mount(t, bridge(null, false));
    expect(screen.queryByTestId("welcome")).toBeNull();
    expect(openMenu().queryByText("Sign in")).toBeNull();
    expect(screen.getByText("Local mode")).toBeTruthy();
    c.dispose();
  });

  it("claims the unowned host on sign-in", async () => {
    const t = new FakeTransport();
    t.handlers["host.claim"] = (p) => ({ owner: { user_id: p.user_id, email: p.email ?? null } });
    const b = bridge(null);
    const c = await mount(t, b);
    fireEvent.click(screen.getByText("Continue with Google"));
    await tick();
    expect(b.signIn).toHaveBeenCalledTimes(1);
    expect(claims(t)).toEqual([{ user_id: "u-me", email: "me@example.com" }]);
    expect(openSettings("Account").getByText("This host is yours")).toBeTruthy();
    expect(screen.queryByTestId("welcome")).toBeNull();
    c.dispose();
  });

  it("never takes over a host another account owns", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ owner: { user_id: "u-other", email: "other@example.com" } });
    const c = await mount(t, bridge(me));
    expect(screen.getByLabelText("This host belongs to another account")).toBeTruthy();
    expect(openSettings("Account").getByTestId("owner-mismatch").textContent).toContain("other@example.com");
    expect(claims(t)).toEqual([]);
    c.dispose();
  });

  it("shows a mismatch when the host refuses the claim", async () => {
    const t = new FakeTransport();
    t.handlers["host.claim"] = () => {
      throw new ZwiimClientError("host_owned", "owned", 409, { owner_email: "other@example.com" });
    };
    const c = await mount(t, bridge(me));
    expect(claims(t)).toHaveLength(1);
    expect(screen.getByLabelText("This host belongs to another account")).toBeTruthy();
    expect(openSettings("Account").getByTestId("owner-mismatch").textContent).toContain("other@example.com");
    c.dispose();
  });

  it("signs out without touching the host", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ owner: me });
    const b = bridge(me);
    const c = await mount(t, b);
    expect(screen.getByText("me@example.com")).toBeTruthy();
    const before = t.calls.length;
    fireEvent.click(openMenu().getByText("Sign out"));
    await tick();
    expect(b.signOut).toHaveBeenCalledTimes(1);
    expect(t.calls.slice(before)).toEqual([]);
    expect(screen.getByText("Local mode")).toBeTruthy();
    expect(openSettings("Account").getByTestId("host-owner").textContent).toBe("Host owner: me@example.com");
    expect(screen.queryByTestId("welcome")).toBeNull();
    c.dispose();
  });

  it("keeps the signed-in account while the host is unreachable", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => {
      throw new ZwiimClientError("network", "down");
    };
    const c = await mount(t, bridge(me));
    expect(screen.getByText("me@example.com")).toBeTruthy();
    expect(claims(t)).toEqual([]);
    c.dispose();
  });

  it("reports a failed sign-in and stays local", async () => {
    const t = new FakeTransport();
    const b = bridge(null);
    b.signIn = vi.fn(async () => {
      throw "Sign-in failed: Denied";
    });
    const c = await mount(t, b);
    fireEvent.click(screen.getByText("Continue with Google"));
    await tick();
    expect(screen.getByTestId("welcome").querySelector("[role=alert]")!.textContent).toBe("Sign-in failed: Denied");
    expect(claims(t)).toEqual([]);
    c.dispose();
  });
});
