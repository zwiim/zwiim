#!/bin/sh
# A production Host is signed as com.zwiim.host by the Zwiim team
# (APPLE_TEAM_ID), and its designated requirement says so. That requirement
# is what the Keychain checks: a Host update signed the same way reads the
# Host's items without prompting; one signed differently cannot.
#
# The power helper is checked the same way as com.zwiim.power-helper (second
# argument), with hardened runtime: launchd runs it as root.
set -eu
host="$1"
id="${2:-com.zwiim.host}"
details=$(codesign -dv --verbose=2 "$host" 2>&1)
echo "$details" | grep -qx "Identifier=$id" || { echo "::error::$host is not signed as $id"; exit 1; }
echo "$details" | grep -qx "TeamIdentifier=$APPLE_TEAM_ID" || { echo "::error::$host is not signed by team $APPLE_TEAM_ID"; exit 1; }
if [ "$id" = com.zwiim.power-helper ]; then
  echo "$details" | grep -Eq "flags=0x[0-9a-f]+\([^)]*runtime" || { echo "::error::$host lacks the hardened runtime"; exit 1; }
fi
requirement=$(codesign -d -r- "$host" 2>&1)
echo "$requirement" | grep -q "identifier \"$id\"" || { echo "::error::$host requirement omits its identifier"; exit 1; }
echo "$requirement" | grep -q "$APPLE_TEAM_ID" || { echo "::error::$host requirement omits team $APPLE_TEAM_ID"; exit 1; }
echo "$host: $id, team $APPLE_TEAM_ID"
