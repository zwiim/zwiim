#!/bin/sh
# Run a command again when it fails, for steps that call Apple's notary
# and ticket servers, whose connections sometimes drop mid-request
# ("The network connection was lost", stapler error 68).
#
#   scripts/release/retry.sh xcrun stapler validate Zwiim.app
set -u
attempt=1
until "$@"; do
  status=$?
  if [ "$attempt" -ge 3 ]; then
    echo "retry: '$1' failed $attempt times (last exit $status)" >&2
    exit "$status"
  fi
  echo "retry: '$1' exited $status; trying again in $((attempt * 30))s" >&2
  sleep $((attempt * 30))
  attempt=$((attempt + 1))
done
