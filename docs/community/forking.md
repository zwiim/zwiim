# Forking Zwiim

Apache-2.0 lets you fork Zwiim and ship your own builds. Keep `LICENSE`, `NOTICE`
and the notices in source files. Use your own name and logo: see
[TRADEMARK.md](../../TRADEMARK.md).

## What to change

| What | Where |
|---|---|
| App identifiers | `com.zwiim.app` in `clients/desktop/src-tauri/tauri.conf.json`, `com.zwiim.mobile` in `clients/mobile/app.json` |
| Host signing identifier | `com.zwiim.host` in `scripts/release/` |
| Release repository | `DEFAULT_RELEASES_URL` in `clients/desktop/src-tauri/src/update.rs`, `runtime/host/src/update/service.rs`, `clients/mobile/src/host/release.ts`; `ZWIIM_RELEASE_REPOSITORY` for `scripts/release/` |
| Update signing key | `ZWIIM_RELEASE_PUBKEY` at build time, with your own key pair |
| Cloud and sign-in | `ZWIIM_CLOUD_URL`, `ZWIIM_SUPABASE_URL`, `ZWIIM_SUPABASE_ANON_KEY` at build time; `EXPO_PUBLIC_ZWIIM_*` for mobile |

A build without a release key never updates itself, and a build without Cloud
settings works locally only.

Run your own relay: see [Self-hosting](../deploy/self-hosting.md).
