#!/usr/bin/env bash
# Run a development Zwiim Cloud on loopback, behind the tunnel tunnel.sh
# opened (or any TLS-terminating proxy at ZWIIM_CLOUD_PUBLIC_URL).
#
#   cargo build -p zwiim-cloud
#   deploy/dev/run-cloud.sh
#
# Settings come from deploy/dev/cloud.env (see cloud.env.example). The public
# URL is ZWIIM_CLOUD_PUBLIC_URL, else the one tunnel.sh recorded. The
# configuration is checked before starting; once up, the Cloud's health is
# checked locally and through the public URL. Ctrl-C stops it gracefully:
# hosts are told it is restarting and reconnect when it is back.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
# shellcheck source=env.sh
. "$here/env.sh"

cloud_bin=${ZWIIM_CLOUD_BIN:-$repo/target/debug/zwiim-cloud}
[ -x "$cloud_bin" ] || die "$cloud_bin not found; run: cargo build -p zwiim-cloud"
if [ -z "${ZWIIM_CLOUD_PUBLIC_URL:-}" ] && [ -f "$state/public-url" ]; then
  ZWIIM_CLOUD_PUBLIC_URL=$(cat "$state/public-url")
  export ZWIIM_CLOUD_PUBLIC_URL
fi
export ZWIIM_CLOUD_DATA_DIR=${ZWIIM_CLOUD_DATA_DIR:-$state/cloud}

"$cloud_bin" --check

"$cloud_bin" &
cloud_pid=$!
trap 'kill -TERM "$cloud_pid" 2>/dev/null' INT TERM

for _ in $(seq 50); do
  curl -fsS "http://$ZWIIM_CLOUD_ADDR/healthz" >/dev/null 2>&1 && break
  kill -0 "$cloud_pid" 2>/dev/null || { wait "$cloud_pid"; exit $?; }
  sleep 0.2
done
public=unreachable
for _ in $(seq 30); do
  if curl -fsS --max-time 5 "$ZWIIM_CLOUD_PUBLIC_URL/healthz" >/dev/null 2>&1; then
    public=ok
    break
  fi
  sleep 2
done
echo "zwiim-cloud: $ZWIIM_CLOUD_PUBLIC_URL/healthz $public (local http://$ZWIIM_CLOUD_ADDR)"

status=0
wait "$cloud_pid" || status=$?
# Interrupted: wait for the graceful stop to finish.
while kill -0 "$cloud_pid" 2>/dev/null; do wait "$cloud_pid" || status=$?; done
exit "$status"
