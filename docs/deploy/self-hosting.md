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

## Relay usage and plan limits

Without a usage database the Cloud counts relay bytes in memory and limits
nothing. To store usage and apply the plans' limits (browser design 15):

1. Apply the migration in `runtime/cloud/supabase/migrations/` to the
   Supabase project, with `supabase db push` or by pasting it into the SQL
   editor. It adds the plans, usage tables and the `zwiim_cloud` role,
   which can only call `add_relay_usage()`.
2. Give that role a login in the SQL editor. Generate the password on your
   machine and keep it out of the repository, issues and shell history:
   ```sql
   alter role zwiim_cloud with login password '<generated>';
   ```
3. Put the URL in an env file only root can read, through the project's
   pooler in session mode (Database settings → Connection string, port 5432):
   ```sh
   sudo install -d -m 0700 /etc/zwiim-cloud
   sudo install -m 0600 /dev/null /etc/zwiim-cloud/cloud.env
   sudoedit /etc/zwiim-cloud/cloud.env
   ```
   ```sh
   ZWIIM_CLOUD_USAGE_DATABASE_URL=postgres://zwiim_cloud.<ref>:<password>@<pooler-host>:5432/postgres
   ```
   The Cloud requires TLS to a database on another machine and checks its
   certificate against the system roots. If that check fails, download the
   project's CA certificate (Database settings → SSL configuration) and set
   `ZWIIM_CLOUD_USAGE_DATABASE_CA` to its path.
4. Load the file in the service, for example with systemd:
   ```ini
   [Service]
   EnvironmentFile=/etc/zwiim-cloud/cloud.env
   ExecStart=/usr/local/bin/zwiim-cloud
   ```
5. Restart and check the startup line says `usage=database`. A
   `usage store unavailable` or `usage sync failed` warning means the Cloud
   can't reach the database; it keeps relaying and limits nothing until it
   can.

Never give the Cloud the service-role key: the `zwiim_cloud` role is all it
needs.

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
