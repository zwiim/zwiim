# Zwiim

**Your Mac runs the agents. Your phone runs your Mac.**

Zwiim runs coding agents on your own machine and lets you steer them from
anywhere. Start Claude Code, Codex and other agents side by side in Git
worktrees, then approve, follow and answer them from your Mac or your phone.

> **Status: release candidate.** Expect rough edges. The Desktop app runs on
> Apple Silicon Macs only for now.

## Why Zwiim

- **Local first.** Your code, agents, terminals and Git stay on your machine.
  Local work never passes through a server, and it keeps working when the
  Cloud is down.
- **Encrypted relay.** Phones reach your Mac through a Noise tunnel, encrypted
  end to end. The relay carries ciphertext only.
- **No network setup.** No port forwarding, public IP or VPN.
- **Parallel by default.** Each session can have its own worktree, with
  conflict resolution and session forks built in.
- **Your agents.** Zwiim drives Claude Code, Codex, Gemini, Cursor, OpenCode and
  others. Bring your own subscriptions and keys.
- **No account needed locally.** Choose "Continue locally" and use it alone on
  your Mac.
- **Open source.** Apache-2.0, including the relay, so you can run your own.

## Install

Download the latest Desktop build from
[Releases](https://github.com/zwiim/zwiim/releases). Then, to use your
phone, sign in on both devices and scan the pairing QR code on your Mac.

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
