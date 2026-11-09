# Codeg source provenance

Zwiim's Host derives from Codeg, and parts of the Desktop UI were adapted from
its web UI. This file records the source and the changes, as the Apache License
2.0 requires.

## Source

| Field | Value |
|---|---|
| Project | Codeg |
| Upstream | https://github.com/xintaofei/codeg |
| Version | 0.31.1 |
| Commit | ac3a336 |
| License | Apache-2.0, copy in `third_party/codeg/LICENSE` |

## What changed

Zwiim copied Codeg's headless Rust core into `runtime/host` and removed its
product and desktop subsystems. Since then Zwiim has renamed its identifiers,
replaced its authentication, updater and remote access, added its own protocol,
identity, pairing, tunnel and Cloud, and changed much of the retained code.
Source files still derived from Codeg carry the line "Modified from upstream
Apache-2.0 source; see NOTICE."

## Retained Codeg core (integrated, headless)

Copied and kept, then modified as described above:

| Area | Zwiim path | Nature |
|---|---|---|
| ACP agent runtime (connection, manager, lifecycle, delegation, tools) | `runtime/host/src/acp/` | Reused |
| Terminal / PTY (portable-pty, seq'd resume) | `runtime/host/src/terminal/` | Reused |
| Filesystem, git, worktree, workspace state, uploads | `runtime/host/src/{web/handlers,workspace_state,git_repo.rs,...}` | Reused |
| Event system (seq/snapshot/replay/reconnect) | `runtime/host/src/web/{event_bridge,ws}.rs` | Reused |
| SQLite / SeaORM persistence (entities, services, migrations) | `runtime/host/src/db/` | Reused (migrations kept **dormant**, not trimmed) |
| MCP host bridge (companion server, now `zwiim-mcp`) + delegation tool groups | `runtime/host/src/bin/zwiim_mcp.rs`, `runtime/host/src/acp/{browser_tools,chat_authoring,work_task_tools}.rs` | Reused (companion renamed `codeg-mcp` → `zwiim-mcp`) |
| Shared command façade (business logic under `web/handlers`) | `runtime/host/src/commands/` | Reused (product commands removed; see below) |
| Skill link engine (`experts`) | `runtime/host/src/commands/experts.rs` | Reused; reduced to the link primitives custom skills share |
| Process supervisor / PID-1 init + exec/supervise runtime constants | `runtime/host/src/supervise.rs`, `runtime/host/src/update/runtime.rs` | Reused |
| Bearer-token + WS-subprotocol auth | `runtime/host/src/web/auth.rs` | Reused for the local loopback token only. Remote devices use paired keys |

## Removed Codeg subsystems

Entire modules deleted from the tree: `work_task`, `automation`,
`pet_state_mapper`, `forge`, `canvas`, `office_tools`, `office_watch`,
`science`, bundled `experts` skills, `backgrounds`, `quick_messages`, `app_update` (minisign
self-updater), `chat_channel` (Telegram, incl. manager/traits/types/commands),
and `web/browser_bridge` (dev-server port bridge). The Tauri/WebKit desktop UI
and its ~200-route command surface are gated out via the absent `tauri-runtime`
feature, so it is compiled out of the headless Host.

`chat_channel` DB **entities** (`db/entities/chat_channel*`) and their services
remain in the tree as dormant persistence, consistent with the "migrations kept
dormant" rule; the runtime subsystem that drove them is gone.
