<h1 align="center">
  <img src="clients/desktop/src-tauri/icons/128x128@2x.png" alt="" width="64" valign="middle" /> Zwiim
</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-6CB4F0?style=flat" alt="License: Apache-2.0" /></a>
  <a href="https://github.com/zwiim/zwiim/releases"><img src="https://img.shields.io/github/v/release/zwiim/zwiim?include_prereleases&style=flat&color=6CB4F0&label=release" alt="Latest release" /></a>
  <img src="https://img.shields.io/badge/macOS-Apple%20Silicon-1E2123?style=flat" alt="macOS on Apple Silicon" />
  <img src="https://img.shields.io/badge/phone-iOS%20%7C%20Android-1E2123?style=flat" alt="Phone app for iOS and Android" />
</p>

<p align="center">
  <strong>Your Mac runs the agents. Your phone runs your Mac.</strong><br/>
  Run Claude Code, Codex and other coding agents side by side on your own machine,
  each in its own Git worktree. Approve, follow and answer them from your Mac or your phone.
</p>

<h3 align="center"><a href="https://github.com/zwiim/zwiim/releases"><ins>Download Zwiim</ins></a></h3>

<p align="center">
  <img src="docs/assets/readme/hero.jpg" alt="The Zwiim Mac app and phone app showing the same agent asking to write a file, with Yes and No buttons on both" width="960" />
</p>

> **Status: release candidate.** Expect rough edges. The Mac app runs on Apple
> Silicon only for now.

## See it in 15 seconds

<p align="center">
  <a href="docs/assets/readme/demo.mp4"><img src="docs/assets/readme/demo.webp" alt="A real screen recording: prompting a new Claude Code session on the Mac, reading a Markdown file and a code file, and opening the diff, while the phone shows the new session working live" width="960" /></a>
  <br/><sub>Recorded live with Recordly on the Mac and the iPhone app, sped up 1.8×. <a href="docs/assets/readme/demo.mp4">Watch in full quality (MP4)</a></sub>
</p>

## Features

<table>
<tr>
<td width="45%" valign="middle">

### Agents side by side

Start several agents at once. Each session gets its own worktree and branch, so
they never step on each other. Zwiim renames the branch to match the work.

</td>
<td width="55%"><img src="docs/assets/readme/parallel-agents.jpg" alt="Three repositories with several worktrees in the sidebar, and an agent's finished summary with passing tests" /></td>
</tr>
<tr>
<td width="45%" valign="middle">

### Your phone runs your Mac

See every session at a glance. The ones waiting for you are at the top, with
what they want to do and a button to review it.

</td>
<td width="55%"><img src="docs/assets/readme/phone.jpg" alt="Three phone screens: the Sessions list with a request waiting, the request with its diff, and a finished session" /></td>
</tr>
<tr>
<td width="45%" valign="middle">

### Approve from anywhere

When an agent asks to edit a file or run a command, the same request shows on
your Mac and your phone. Answer it on whichever is closer.

</td>
<td width="55%"><img src="docs/assets/readme/approve-anywhere.jpg" alt="The same permission request on the Mac and on the phone" /></td>
</tr>
<tr>
<td width="45%" valign="middle">

### Review every change

Read each file's diff against your base branch before anything is committed,
in unified or split view.

</td>
<td width="55%"><img src="docs/assets/readme/review-diffs.jpg" alt="A file diff with added lines highlighted, next to the list of changed files" /></td>
</tr>
<tr>
<td width="45%" valign="middle">

### Plans wait for your OK

In plan mode the agent writes a plan first. Approve it, adjust it, or hand it to
a new session before any code is written.

</td>
<td width="55%"><img src="docs/assets/readme/plan-review.jpg" alt="An agent's plan with Approve, Hand off and Abandon buttons" /></td>
</tr>
<tr>
<td width="45%" valign="middle">

### Pair once, then go

Sign in on both devices and scan the code on your Mac. No port forwarding,
public IP or VPN. The relay carries end-to-end encrypted traffic only.

</td>
<td width="55%"><img src="docs/assets/readme/pairing.jpg" alt="Phone screens for signing in, confirming the Mac to pair with, and the connected Mac" /></td>
</tr>
</table>

<sub>All screenshots are from the real apps, running real Claude Code sessions on sample projects.</sub>

## Why Zwiim

- **Local first.** Your code, agents, terminals and Git stay on your machine.
  Local work never passes through a server, and it keeps working when the
  Cloud is down.
- **Encrypted relay.** Phones reach your Mac through a Noise tunnel, encrypted
  end to end. The relay carries ciphertext only.
- **Your agents.** Zwiim drives Claude Code, Codex, Gemini, Cursor, OpenCode and
  others. Bring your own subscriptions and keys.
- **No account needed locally.** Choose "Continue locally" and use it alone on
  your Mac.
- **Open source.** Apache-2.0, including the relay, so you can run your own.

## Install

1. Download the Mac app from [Releases](https://github.com/zwiim/zwiim/releases).
2. Get the phone app: the Android APK is on the same Releases page. The iOS
   app is not public yet.
3. Sign in on both devices, then scan the pairing code on your Mac.

## How it works

```
LOCAL:   Desktop ──────────────────────────► Host ──► agents, Git, files, terminals
REMOTE:  Phone ──► Zwiim Cloud (relay) ────► Host ──► agents, Git, files, terminals
```

- **Host** (`runtime/host`): the source of truth for workspaces, sessions,
  agents, terminals, files and Git. It runs on your machine.
- **Cloud** (`runtime/cloud`): accounts, presence and the encrypted relay. It
  never runs your work and cannot read relayed traffic.
- **Clients** (`clients/desktop`, `clients/mobile`, `clients/sdk`): they send
  commands and render what the Host reports. They hold no authoritative state.

## Build from source

You need Rust (stable), Node 20+ and Xcode Command Line Tools.

```sh
npm ci
npm run build --workspace clients/desktop
cargo build -p zwiim-host
cd clients/desktop && npm run tauri dev
```

The app works locally without any Cloud settings. For remote access, run your
own relay: see [Self-hosting](docs/deploy/self-hosting.md). To ship your own
builds, see [Forking](docs/community/forking.md).

Tests:

```sh
cargo test -p zwiim-host -p zwiim-cloud -p zwiim-desktop
npm test --workspace clients/desktop
```

On a Mac, the LAN tests need the firewall to allow each rebuilt test binary.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), and please
report security problems privately as [SECURITY.md](SECURITY.md) describes.

## License

Zwiim is licensed under the [Apache License 2.0](LICENSE). See [NOTICE](NOTICE)
for attribution. The name and logo are covered by [TRADEMARK.md](TRADEMARK.md).
