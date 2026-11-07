# Self-hosting the relay

Zwiim Cloud (`runtime/cloud`) handles accounts, host registration, presence and
the encrypted relay. You need it only for remote access. Local work never uses
it.

## What you need

1. A server with a public HTTPS address.
2. A Supabase project for sign-in, with the Google provider on. Setup is in
   [Supabase](local-cloud.md#supabase).
3. A Rust toolchain to build `zwiim-cloud`.

With asymmetric signing keys on, the Cloud needs only Supabase's public values.
Never give it the service-role key.

## Run it

```sh
cargo build --release -p zwiim-cloud
```

Then set at least these, and start `target/release/zwiim-cloud`:

```sh
ZWIIM_CLOUD_PUBLIC_URL=https://relay.example.com
ZWIIM_CLOUD_DATA_DIR=/var/lib/zwiim-cloud
ZWIIM_CLOUD_ADDR=0.0.0.0:443
ZWIIM_CLOUD_TLS_CERT=/etc/zwiim/fullchain.pem
ZWIIM_CLOUD_TLS_KEY=/etc/zwiim/privkey.pem
ZWIIM_CLOUD_SUPABASE_URL=https://<ref>.supabase.co
```

Or keep the default loopback address and put an HTTPS reverse proxy in front.
All settings are in [Cloud settings](local-cloud.md#cloud-settings).

## Point the apps at it

Build Desktop with `ZWIIM_CLOUD_URL`, `ZWIIM_SUPABASE_URL` and
`ZWIIM_SUPABASE_ANON_KEY`, and mobile with their `EXPO_PUBLIC_ZWIIM_*` versions.
See [Forking](../community/forking.md).

## Operating it

- It is a single process, with its registry on local disk under the data
  directory. Back that directory up. It holds host registrations and public keys,
  not your code.
- Behind a proxy on another machine, every client appears as one address, so
  raise `ZWIIM_CLOUD_MAX_CONNECTS_PER_IP`.
- If the relay goes down, remote access stops until it returns. Local work
  continues.
