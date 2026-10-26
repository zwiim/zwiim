import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, ZwiimClientError, type GithubAuth } from "@zwiim/sdk";
import { Workbench } from "@/App";
import { openLink, type NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { GITHUB_POLL_MS } from "@/panels/Github";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openMenu, openSettings } from "./ui";

vi.mock("@/lib/bridge", async (actual) => ({
  ...(await actual<typeof import("@/lib/bridge")>()),
  openLink: vi.fn(async () => {}),
}));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.mocked(openLink).mockClear();
});
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

const CODE: GithubAuth = { state: "signing_in", code: "ABCD-1234", url: "https://github.com/login/device" };

/** A Host whose GitHub CLI answers `github.auth_status` with `status()`. */
function githubHost(status: () => GithubAuth) {
  const t = new FakeTransport();
  t.handlers["github.auth_status"] = status;
  return t;
}

async function mount(t: FakeTransport) {
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

async function openGithub() {
  const dialog = openSettings("GitHub");
  await tick();
  return dialog;
}

const statusCalls = (t: FakeTransport) => t.calls.filter((c) => c.op === "github.auth_status").length;

describe("Settings → GitHub", () => {
  it("names the account the Host's GitHub CLI is signed in to", async () => {
    const t = githubHost(() => ({ state: "signed_in", login: "Ashish1101" }));
    const c = await mount(t);
    const dialog = await openGithub();
    expect(dialog.getByTestId("github-signed-in").textContent).toBe("Signed in to GitHub as Ashish1101");
    expect(dialog.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
    c.dispose();
  });

  it("says when the Host has no GitHub CLI", async () => {
    const c = await mount(githubHost(() => ({ state: "unavailable" })));
    const dialog = await openGithub();
    expect(dialog.getByText("GitHub CLI isn't installed on this Mac.")).toBeTruthy();
    expect(dialog.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
    c.dispose();
  });

  it("signs in: shows the code to copy, opens GitHub, and looks every 2 s until it settles", async () => {
    let state: GithubAuth = { state: "signed_out" };
    const t = githubHost(() => state);
    t.handlers["github.sign_in"] = () => (state = CODE);
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const c = await mount(t);
    const dialog = await openGithub();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });

    fireEvent.click(dialog.getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "github.sign_in").map((x) => x.params)).toEqual([{}]);
    const box = within(dialog.getByTestId("github-code"));
    expect(box.getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");
    expect(box.getByRole("status").textContent).toBe("Waiting for GitHub…");

    fireEvent.click(box.getByTitle("Copy code"));
    await tick();
    expect(writeText).toHaveBeenCalledWith("ABCD-1234");
    expect(box.getByTitle("Copy code").textContent).toContain("Copied");

    fireEvent.click(box.getByRole("button", { name: /Open GitHub/ }));
    expect(openLink).toHaveBeenCalledWith("https://github.com/login/device");

    const before = statusCalls(t);
    await act(async () => void vi.advanceTimersByTime(GITHUB_POLL_MS));
    await tick();
    expect(statusCalls(t)).toBe(before + 1);
    await act(async () => void vi.advanceTimersByTime(GITHUB_POLL_MS));
    await tick();
    expect(statusCalls(t)).toBe(before + 2);

    state = { state: "signed_in", login: "Ashish1101" };
    await act(async () => void vi.advanceTimersByTime(GITHUB_POLL_MS));
    await tick();
    expect(dialog.getByTestId("github-signed-in").textContent).toBe("Signed in to GitHub as Ashish1101");
    expect(dialog.queryByTestId("github-code")).toBeNull();
    // Settled: no more looking.
    const settled = statusCalls(t);
    await act(async () => void vi.advanceTimersByTime(GITHUB_POLL_MS * 5));
    await tick();
    expect(statusCalls(t)).toBe(settled);
    c.dispose();
  });

  it("Cancel stops the sign-in and the looking", async () => {
    let state: GithubAuth = CODE;
    const t = githubHost(() => state);
    t.handlers["github.cancel_sign_in"] = () => (state = { state: "signed_out" });
    const c = await mount(t);
    const dialog = await openGithub();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    // A sign-in already under way shows its code again.
    expect(dialog.getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");

    fireEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "github.cancel_sign_in").map((x) => x.params)).toEqual([{}]);
    expect(dialog.queryByTestId("github-code")).toBeNull();
    expect(dialog.getByRole("button", { name: "Sign in to GitHub" })).toBeTruthy();
    const after = statusCalls(t);
    await act(async () => void vi.advanceTimersByTime(GITHUB_POLL_MS * 3));
    await tick();
    expect(statusCalls(t)).toBe(after);
    c.dispose();
  });

  it("after three looks in a row fail, says the Host isn't connected; keeps looking and clears on an answer", async () => {
    let state: GithubAuth = { state: "signed_out" };
    let failing = false;
    const t = githubHost(() => {
      if (failing) throw new ZwiimClientError("timeout", "no answer");
      return state;
    });
    t.handlers["github.sign_in"] = () => (state = CODE);
    const c = await mount(t);
    const dialog = await openGithub();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    fireEvent.click(dialog.getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    const look = async (times = 1) => {
      for (let i = 0; i < times; i++) {
        await act(async () => void vi.advanceTimersByTime(GITHUB_POLL_MS));
        await tick();
      }
    };

    failing = true;
    await look(2);
    expect(dialog.queryByText("Not connected to the Host")).toBeNull();
    await look();
    expect(dialog.getByText("Not connected to the Host")).toBeTruthy();
    // Still looking, and the code stays up.
    const before = statusCalls(t);
    await look();
    expect(statusCalls(t)).toBe(before + 1);
    expect(dialog.getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");

    failing = false;
    await look();
    expect(dialog.queryByText("Not connected to the Host")).toBeNull();
    expect(dialog.getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");
    c.dispose();
  });

  it("shows why a sign-in didn't finish, once, and lets it start again", async () => {
    let state: GithubAuth = { state: "signed_out" };
    const t = githubHost(() => state);
    t.handlers["github.sign_in"] = () =>
      (state = { state: "signed_out", error: "GitHub didn't give a sign-in code. Try again." });
    const c = await mount(t);
    const dialog = await openGithub();
    fireEvent.click(dialog.getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    expect(dialog.getByRole("alert").textContent).toBe("GitHub didn't give a sign-in code. Try again.");

    t.handlers["github.sign_in"] = () => (state = CODE);
    fireEvent.click(dialog.getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    expect(dialog.queryByRole("alert")).toBeNull();
    expect(dialog.getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");
    c.dispose();
  });

  it("is not offered by a Host that can't sign GitHub in", async () => {
    const t = githubHost(() => ({ state: "signed_in", login: "Ashish1101" }));
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "github.auth") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("GitHub")).toBeNull();
    expect(nav.getByText("Agents")).toBeTruthy();
    expect(statusCalls(t)).toBe(0);
    c.dispose();
  });
});
