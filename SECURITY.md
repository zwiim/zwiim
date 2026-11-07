# Security

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting
on this repository, or email hello@zwiim.com. Include steps to reproduce and the
version you tested.

We aim to acknowledge reports within three working days. Please give us
reasonable time to fix a problem before you disclose it.

Only the latest release is supported.

## What Zwiim protects, and what it does not

**A paired device has full control of your machine.** This is the product: a
phone can run agents and open a terminal as your user. Pair only devices you
trust.

- **Revoking a device** ends its access and its open tunnels. It cannot undo what
  that device already did. If a device may have been misused, treat the machine
  as exposed: check for changes and rotate its credentials.
- **Agents run as you.** Zwiim confines writes made through its own file channel,
  but an agent's shell tools have your user's access. Review what you approve.
- **The relay is untrusted by design.** Traffic between device and Host is
  encrypted end to end. A Cloud operator can see accounts, host IDs, presence and
  connection metadata, but not your code or terminals.
- **The Host listens on loopback by default.** Setting `ZWIIM_HOST` exposes its
  local API, which is plain HTTP, to your network. Do not do this on a network
  you do not control.

On your Mac, credentials live in the OS Keychain. With
`ZWIIM_HOST_KEY_STORE=file`, a headless Host keeps them in owner-only files.
