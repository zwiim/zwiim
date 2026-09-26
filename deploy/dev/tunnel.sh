#!/usr/bin/env bash
# Expose a development Zwiim Cloud listening on loopback through a Cloudflare
# Tunnel, and record its public URL for run-cloud.sh.
#
#   deploy/dev/tunnel.sh                 # quick tunnel: a new *.trycloudflare.com URL each run
#   deploy/dev/tunnel.sh named <tunnel>  # named tunnel: ZWIIM_CLOUD_PUBLIC_URL's stable hostname
#
# Runs in the foreground until Ctrl-C; the public URL is written to
# $ZWIIM_DEV_DIR/public-url (default ~/.zwiim-dev) and removed on exit.
# Cloudflare is only the tunnel: nothing in Zwiim depends on it, and the
# Cloud behind it can be stopped and restarted while the URL stays the same.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=env.sh
. "$here/env.sh"

mode=${1:-quick}
command -v cloudflared >/dev/null || die "cloudflared is not installed (brew install cloudflared)"
origin="http://${ZWIIM_CLOUD_ADDR}"
log="$state/cloudflared.log"
: >"$log"

cleanup() {
  rm -f "$state/public-url"
  [ -n "${tunnel_pid:-}" ] && kill "$tunnel_pid" 2>/dev/null && wait "$tunnel_pid" 2>/dev/null
  true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

case $mode in
  quick)
    # A private HOME: an existing ~/.cloudflared/config.yaml (another tunnel's
    # ingress rules) would otherwise take over this tunnel's routing.
    mkdir -p "$state/cloudflared-home"
    HOME="$state/cloudflared-home" cloudflared tunnel --no-autoupdate --url "$origin" >>"$log" 2>&1 &
    tunnel_pid=$!
    url=
    for _ in $(seq 60); do
      url=$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' "$log" | head -1 || true)
      [ -n "$url" ] && break
      kill -0 "$tunnel_pid" 2>/dev/null || die "cloudflared exited; see $log"
      sleep 1
    done
    [ -n "$url" ] || die "no quick-tunnel URL after 60s; see $log"
    ;;
  named)
    name=${2:-}
    [ -n "$name" ] || die "usage: tunnel.sh named <tunnel-name>"
    url=${ZWIIM_CLOUD_PUBLIC_URL:-}
    [[ $url == https://* ]] || die "set ZWIIM_CLOUD_PUBLIC_URL to the tunnel's https:// hostname"
    hostname=${url#https://}
    hostname=${hostname%/}
    # Its own config file: your ~/.cloudflared/config.yaml is left alone.
    # The tunnel's credentials are found in ~/.cloudflared as usual.
    config="$state/cloudflared-$name.yml"
    cat >"$config" <<EOF
tunnel: $name
ingress:
  - hostname: $hostname
    service: $origin
  - service: http_status:404
EOF
    cloudflared tunnel --no-autoupdate --config "$config" run "$name" >>"$log" 2>&1 &
    tunnel_pid=$!
    ;;
  *) die "unknown mode $mode (quick or named <tunnel>)" ;;
esac

echo "$url" >"$state/public-url"
echo "tunnel: $url -> $origin (log: $log)"
echo "start the Cloud with deploy/dev/run-cloud.sh; Ctrl-C here closes the tunnel"
wait "$tunnel_pid"
