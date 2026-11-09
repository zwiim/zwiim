# Zwiim: notes for coding agents

Zwiim runs coding agents on the user's machine and lets them steer it from
other devices. Read `README.md` for the layout and `CONTRIBUTING.md` for checks.

## Working rules

- Keep each change to one purpose. Inspect the existing code first and find the
  smallest place to make it.
- Run the checks in `CONTRIBUTING.md` before calling work done.
- Do not add speculative abstractions or future infrastructure.
- Visual changes follow `DESIGN.md`.

## Architecture rules

**The Host is the authority.** The Host (`runtime/host`) owns workspaces,
sessions, agents, terminals, files and Git. Clients send commands and render
what the Host reports; they hold no authoritative state.

**Local work never depends on the Cloud.**

```
LOCAL:  Desktop -> Host -> local resources
REMOTE: phone   -> Zwiim Cloud (relay) -> Host -> local resources
```

The Cloud (`runtime/cloud`) only handles accounts, presence and the encrypted
relay. Never route a local operation through it.

**Clients use the Zwiim protocol only** (`/zwiim/v1`). Internal APIs, agent
protocols (ACP), database tables and internal event types never reach clients.
Add protocol operations only when a feature needs one, and bump the protocol
revision in `runtime/host/src/protocol/compat.rs` and `clients/sdk`.

**Zwiim naming.** Zwiim-owned code uses Zwiim names. Keep the names of external
protocols, libraries and vendor APIs as they are.

## Security rules

- Credentials live in the OS keychain, never in SQLite or plain files, except
  under `ZWIIM_HOST_KEY_STORE=file`, which a headless Host opts into.
- The Host listens on loopback by default.
- No copyleft dependencies. `node scripts/licenses/notices.mjs --check` enforces
  this.
- Files modified from upstream source carry the line "Modified from upstream
  Apache-2.0 source; see NOTICE." Keep it, and add it to any newly derived file.
