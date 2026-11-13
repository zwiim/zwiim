# Development deployment: Zwiim Cloud on your Mac, reachable from the Internet

This runs `zwiim-cloud` on the same Mac as the Zwiim Host, behind a Cloudflare
Tunnel, so that a Desktop on another network can reach the Host through it.
Only development infrastructure is involved. You do not need a VPS, a public
IP, port forwarding, or an inbound firewall rule.

```
Machine A (your Mac)                                   Machine B (anywhere)
  Desktop ─local─► Host ──outbound WSS──┐                Desktop
                                        ▼                   │
  zwiim-cloud (127.0.0.1:8443) ◄── cloudflared ◄══ Internet ══╛  https://<tunnel>/
```

The Cloud handles accounts, host registration, presence, tickets, and the
encrypted relay. The Host stays the authority for workspaces, sessions,
terminals, files, and Git. Local work never passes through the Cloud and
keeps working when the Cloud is down.

Cloudflare is only a TLS-terminating tunnel. Zwiim has no Cloudflare-specific
code, so any HTTPS reverse proxy in front of the loopback listener works the
same way. The Cloud can also terminate TLS itself (see the settings below).

## What you need

| | |
|---|---|
| Rust toolchain, Node 20+ | to build the Cloud, the Host, and the Desktop |
| `cloudflared` | `brew install cloudflared`. A quick tunnel needs no Cloudflare account. |
| A Supabase project | for sign-in. Google provider enabled; see [Supabase](#supabase). |

### Supabase

Values used:

| Value | Where it goes | Public or secret |
|---|---|---|
| Project URL `https://<ref>.supabase.co` | Cloud `ZWIIM_CLOUD_SUPABASE_URL`; Desktop `ZWIIM_SUPABASE_URL` | public |
| Anon (publishable) key | Desktop `ZWIIM_SUPABASE_ANON_KEY` | public by design |
| JWT signing keys (asymmetric, ES256/RS256) | nowhere: the Cloud fetches the public keys from `<project>/auth/v1/.well-known/jwks.json` | public keys only |
| Legacy HS256 JWT secret | Cloud `ZWIIM_CLOUD_JWT_SECRET`, only for projects without asymmetric keys | **secret** |
| Service-role key | nowhere. Zwiim never uses it. | **secret** |
| Google OAuth client ID and secret | Supabase dashboard only (Auth → Providers → Google) | the secret is **secret** |

In the Supabase dashboard:

1. Auth → Providers → enable Google.
2. Auth → URL Configuration → Redirect URLs → add `http://127.0.0.1:*/callback`.
   The Desktop receives the sign-in on a one-time loopback port.
3. Settings → JWT Keys → use asymmetric signing keys, so the Cloud needs no
   secret.

Never commit the HS256 secret, the service-role key, Google or Cloudflare
credentials, or anything under `~/.zwiim-dev`. `deploy/dev/cloud.env` is
git-ignored for this reason.

## A. Start the Cloud

```sh
cargo build -p zwiim-cloud -p zwiim-host
cp deploy/dev/cloud.env.example deploy/dev/cloud.env
# edit deploy/dev/cloud.env: set ZWIIM_CLOUD_SUPABASE_URL
```

Start the tunnel first (step C) so that the Cloud knows its public URL, then:

```sh
deploy/dev/run-cloud.sh
```

`run-cloud.sh` does four things:

1. Validates the settings (`zwiim-cloud --check`). If a setting is wrong, it
   exits with a message that names the variable.
2. Starts the Cloud on `127.0.0.1:8443`.
3. Checks `/healthz` locally and through the public URL.
4. Logs to the terminal.

The host registry is kept in `ZWIIM_CLOUD_DATA_DIR`, by default
`~/.zwiim-dev/cloud/registry.json`, with mode 0600. It holds only public keys
and account IDs.

## B. Start the Host

The Desktop starts the Host itself. In development, point it at the binary
you built:

```sh
export ZWIIM_HOST_BIN=$PWD/target/debug/zwiim-host
```

The Host needs no Cloud setting of its own. It gets the Cloud URL when you
enable remote access (step F). It then keeps its own outbound connection to
the Cloud, reconnects with backoff (1 s to 60 s), and listens only on
127.0.0.1. The exception is **Local network access**, which is off by
default. See "Direct connection on the same network" under step G.

## C. Start the tunnel

**Quick tunnel** (no account; a new URL every time it starts):

```sh
deploy/dev/tunnel.sh
# tunnel: https://<random>.trycloudflare.com -> http://127.0.0.1:8443
```

**Named tunnel** (stable hostname; needs your Cloudflare account and a domain):

```sh
cloudflared tunnel login                     # once
cloudflared tunnel create zwiim-dev          # once
cloudflared tunnel route dns zwiim-dev cloud-dev.example.com   # once
ZWIIM_CLOUD_PUBLIC_URL=https://cloud-dev.example.com deploy/dev/tunnel.sh named zwiim-dev
```

`tunnel.sh` writes the public URL to `~/.zwiim-dev/public-url`, where
`run-cloud.sh` picks it up. It never touches `~/.cloudflared/config.yaml`:

- A quick tunnel runs with a private `HOME`, so another tunnel's ingress
  rules cannot take it over.
- A named tunnel runs with its own generated config file.

Stopping and restarting the Cloud does not change the URL. Only restarting
a quick tunnel does.

## D. Configure the public URL

| Component | How it learns the URL |
|---|---|
| Cloud | `ZWIIM_CLOUD_PUBLIC_URL`, else `~/.zwiim-dev/public-url` |
| Desktop (both machines) | `ZWIIM_CLOUD_URL` when the app starts (else the URL built in) |
| Host | from the Desktop when you choose **Enable**; stored by the Host |

If the URL changes, for example because the quick tunnel restarted:

1. Restart the Desktops with the new `ZWIIM_CLOUD_URL`.
2. On Machine A, open Devices → Remote access. It shows "Enabled on
   `<old URL>`; this app uses `<new URL>`". Choose **Move to this Cloud**.

The Host keeps its identity, and paired devices stay paired.

Health check:

```sh
curl https://<tunnel>/healthz     # "ok" once the configuration and registry are loaded
```

## E. Start the Desktop

On each machine:

```sh
cd clients/desktop
export ZWIIM_CLOUD_URL=https://<tunnel>
export ZWIIM_SUPABASE_URL=https://<ref>.supabase.co
export ZWIIM_SUPABASE_ANON_KEY=<anon key>
npm install && npm run tauri dev
```

Machine B needs no Host, and it needs neither the tunnel nor the Cloud to
be local.

## F. Pair

1. On Machine A: **Sign in** with Google, then open Devices → Remote access →
   **Enable**.
   - The Desktop fetches a single-use enrollment token for your account, and
     the Host redeems it.
   - The status becomes "On".
2. On Machine B: **Sign in** with the same Google account. The Host appears in
   the host list, online.
3. On Machine A: open Devices → **Pair a device**. A QR code and its text
   appear.
   - They carry the Host's ID, its public key, a one-time code, and an expiry.
   - They carry no address, token, or reusable secret.
4. On Machine B: open the host list → **Pair with a host…**, then paste the
   text.
   - Machine B pins the Host's key.
   - The one-time code cannot be used again.

## G. Verify

From Machine B, on the remote Host:

1. Open the workspace tree and a file.
2. Check Git status and the Git log.
3. Open a terminal.
4. Start an agent session, send a follow-up, and read its history.

Then check recovery and access control:

- **Relay interruption:** turn Wi-Fi off and on. Machine B reconnects and
  replays missed events without duplicates.
- **Cloud restart:** stop and start the Cloud (step H). The Host reconnects
  on its own, with no new enrollment, and Machine B follows.
- **Host restart:** quit the Host on Machine A and restart it. Machine B shows
  it offline, then live again.
- **Revoke:** on Machine A, open Devices → **Revoke** for Machine B. Machine B
  is dropped at once and cannot reconnect.
- **Wrong account:** sign in on Machine B with another Google account. The
  Host is not listed.
- **Cloud down:** with the Cloud stopped, Machine A keeps working locally.

### Direct connection on the same network

On Machine A, open Devices → **Local network access** → **Turn on**. The Host
then does two things:

- It accepts the device tunnel on a TCP port on its IPv4 interfaces. Set
  `ZWIIM_LAN_PORT` to pin the port; otherwise the system picks it. macOS may
  ask whether to allow incoming connections.
- It advertises `<host_id>._zwiim._tcp.local.` over mDNS. The advertisement
  carries only the host ID, the port and the Host's public key.

The port carries only the encrypted tunnel. Only devices that are already
paired get through, and a revoked device is refused there too.

On Machine B, the connection menu under the Host's status sets the mode:

- **Automatic** (the default): direct when the Host is found on this
  network, otherwise through the Cloud. The status shows **Connected
  locally** or **Connected through Zwiim Cloud**.
- **Local only**: never contacts the Cloud.
- **Cloud only**: never looks on the network.

To check fallback, turn Local network access off on Machine A, or cut the
connection. Machine B reconnects through the Cloud. It moves back to the
direct path the next time it reconnects, never while a connection is
healthy.

Automated version, with the real Host, Cloud and mDNS on one Mac:

```sh
cargo build -p zwiim-cloud -p zwiim-host
cd clients/sdk
ZWIIM_LAN_IT=1 npx vitest run test/lan-integration.test.ts
```

Automated version: the same steps run against the real binaries, through
the deployment scripts and a real quick tunnel. The test uses a local
stand-in for Supabase's JWKS endpoint and a scripted agent, so it needs no
credentials.

```sh
cargo build -p zwiim-cloud -p zwiim-host
cd clients/desktop
ZWIIM_REMOTE_E2E=1 npx vitest run test/remote-e2e.test.ts
```

Options:

- `ZWIIM_REMOTE_E2E_TUNNEL=0` runs on loopback instead of through a tunnel.
- `ZWIIM_REMOTE_E2E_REAL_AGENT=1` adds one short turn with the real Claude
  Code agent. It needs `claude-agent-acp` on `PATH` and a signed-in Claude
  Code.

## H. Stop and restart the Cloud

Press Ctrl-C in the `run-cloud.sh` terminal. The shutdown is graceful:

1. The Cloud stops accepting connections.
2. It tells connected Hosts that it is restarting (WebSocket close 1012).
3. It drains for up to 5 s, then logs `[cloud] stopped`.

Start it again with `deploy/dev/run-cloud.sh`. Enrolled Hosts come back
online without enrolling again, because the registry persists.

What is lost on a restart, by design, is only short-lived state:

- presence
- enrollment tokens and connection tickets that were not yet used
- relays in progress, which the Desktop re-establishes on its own

While the Cloud is down:

- Remote Desktops show the Host as unreachable and keep retrying.
- The Host keeps retrying with backoff.
- The tunnel answers 502.

## I. Clean up

```sh
# Ctrl-C run-cloud.sh, then Ctrl-C tunnel.sh (a quick tunnel disappears with it)
cloudflared tunnel delete zwiim-dev    # named tunnel only; remove its DNS record in Cloudflare
rm -rf ~/.zwiim-dev                    # Cloud registry and tunnel state
```

On Machine A, open Devices → **Revoke** for devices you no longer use. To
stop remote access to the Host altogether, stop its Cloud: the Host then
only retries in the background.

## Cloud settings

All settings come from the environment; `run-cloud.sh` also reads
`deploy/dev/cloud.env`. Empty values count as unset.

| Variable | Default | |
|---|---|---|
| `ZWIIM_CLOUD_PUBLIC_URL` | required | https origin clients use (http only for loopback) |
| `ZWIIM_CLOUD_DATA_DIR` | required | registry directory; created 0700 |
| `ZWIIM_CLOUD_ADDR` | `127.0.0.1:8443` | a non-loopback address requires TLS |
| `ZWIIM_CLOUD_TLS_CERT`, `ZWIIM_CLOUD_TLS_KEY` | unset | PEM files; both or neither |
| `ZWIIM_CLOUD_SUPABASE_URL` | unset | verify ES256/RS256 tokens with the project's JWKS; issuer `<url>/auth/v1` |
| `ZWIIM_CLOUD_JWT_SECRET` | unset | **secret**; HS256 verification. At least one of the two is required. |
| `ZWIIM_CLOUD_JWT_AUDIENCE` | `authenticated` | |
| `ZWIIM_CLOUD_USAGE_DATABASE_URL` | unset | **secret**; Postgres URL for the `zwiim_cloud` role (`runtime/cloud/supabase/migrations/`), through Supabase's pooler in session mode. Relay bytes per account are stored there once a minute. Unset: counted in memory only, nothing limited |
| `ZWIIM_CLOUD_USAGE_DATABASE_CA` | system roots | PEM file the usage database's certificate is checked against |
| `ZWIIM_CLOUD_MAX_CONNECTIONS` | `1024` | open sockets; extra connections are closed on arrival |
| `ZWIIM_CLOUD_MAX_RELAYS` | `256` | concurrent relays; more are refused with close 1013, and clients retry |
| `ZWIIM_CLOUD_MAX_CONNECTS_PER_IP` | `120` | new connections per client address per minute; IPv6 counts by /64. Loopback peers are exempt, so a local tunnel or proxy is unaffected. A proxy on another machine counts as one client, so raise it there |
| `ZWIIM_CLOUD_MAX_REQUESTS_PER_ACCOUNT` | `300` | authenticated requests per account per minute; more get a 429 |
| `ZWIIM_CLOUD_LOG` | `RUST_LOG`, else `info` | tracing filter |
| `ZWIIM_CLOUD_LOG_FORMAT` | `text` | `text` or `json` |

Fixed limits:

- relay frames: 128 KiB
- control messages: 8 KiB
- heartbeat: every 15 s, presence timeout 45 s
- relays per Host: 32

The Cloud answers only `/healthz` and `/v1/*`. There is no `/api` surface.

No CORS configuration is needed:

- The Desktop makes its Cloud HTTP calls from its native side.
- Its WebSockets are not subject to CORS.

What the logs contain: host IDs, relay IDs, close reasons, and the public
configuration summary. They never contain tokens, keys, pairing codes, or
relayed bytes, which are end-to-end encrypted between device and Host.

### Limitations of this setup

- A quick tunnel's URL changes on every start. Use a named tunnel for a
  stable URL.
- The Cloud is a single process with its registry on local disk. That is
  fine for development; it is not a production deployment.
