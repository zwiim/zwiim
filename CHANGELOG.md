# Changelog

Every Zwiim release (Host and Desktop share the version) has a section here;
its text becomes the release notes and the `notes` of `release.json`
(`scripts/release/notes.mjs`). Each section carries all eight headings, in
this order; write "None." under one with nothing to say. Mobile store builds
have their own versions and are listed under Compatibility.

## [Unreleased]

## [0.1.0] - 2026-09-28

### Added
- Zwiim Host Runtime: workspaces, worktrees, sessions, terminals, files and Git on your Mac, with the Zwiim Protocol V1 for clients.
- Zwiim Desktop for macOS (Apple Silicon), with the Host bundled.
- Signed releases: a signed `release.json` describes each release; the Host verifies and installs its own updates, tries each new version and rolls back one that fails.
- Desktop updates through the signed Tauri updater. A Desktop update brings its Host without a second download.
- Managed Claude and Codex adapters, pinned per release and installed from the npm registry with signature checks.
- Protocol compatibility: clients and Hosts agree on a protocol revision in `host.hello`; an app that is too old or too new says which side to update.

### Changed
- None.

### Fixed
- None.

### Security
- Release artifacts are trusted only through the release key compiled into each build, then by SHA-256 and the Apple Developer ID signature.
- A Host never runs an unverified download, never installs an older version, and refuses a release whose manifest changed after it was first seen.

### Compatibility
- Protocol `zwiim/1`, revision 1; clients from revision 1.
- Mobile: no store build yet.

### Provider changes
- Claude adapter 0.81.2 (`@agentclientprotocol/claude-agent-acp`).
- Codex adapter 1.12.0 (`@agentclientprotocol/codex-acp`).
- Other agents are experimental and not supported in this release.

### Migration notes
- First release: nothing to migrate. Before any later migration the Host snapshots its database, and a failed update restores it.

### Known limitations
- macOS on Apple Silicon only.
- Only the stable channel is offered as an update.
- Updates apply when you restart the Host (Desktop → About & updates, or Update Host on Mobile).
