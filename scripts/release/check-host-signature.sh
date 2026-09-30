#!/bin/sh
# A production Host is signed as dev.zwiim.host by the Zwiim team
# (APPLE_TEAM_ID), and its designated requirement says so. That requirement
# is what the Keychain checks: a Host update signed the same way reads the
# Host's items without prompting; one signed differently cannot.
set -eu
host="$1"
details=$(codesign -dv --verbose=2 "$host" 2>&1)
echo "$details" | grep -qx "Identifier=dev.zwiim.host" || { echo "::error::$host is not signed as dev.zwiim.host"; exit 1; }
echo "$details" | grep -qx "TeamIdentifier=$APPLE_TEAM_ID" || { echo "::error::$host is not signed by team $APPLE_TEAM_ID"; exit 1; }
requirement=$(codesign -d -r- "$host" 2>&1)
echo "$requirement" | grep -q 'identifier "dev.zwiim.host"' || { echo "::error::$host requirement omits its identifier"; exit 1; }
echo "$requirement" | grep -q "$APPLE_TEAM_ID" || { echo "::error::$host requirement omits team $APPLE_TEAM_ID"; exit 1; }
echo "$host: dev.zwiim.host, team $APPLE_TEAM_ID"
