# Shared by the deploy/dev scripts: settings from deploy/dev/cloud.env (or
# $ZWIIM_CLOUD_ENV), never overriding variables already in the environment.
# Only ${HOME} is expanded in values; nothing in the file is executed.

die() {
  echo "error: $*" >&2
  exit 1
}

env_file=${ZWIIM_CLOUD_ENV:-$here/cloud.env}
if [ -f "$env_file" ]; then
  while IFS= read -r line || [ -n "$line" ]; do
    case $line in '' | \#*) continue ;; esac
    key=${line%%=*}
    [[ $key =~ ^[A-Z_][A-Z0-9_]*$ ]] || die "$env_file: not a KEY=value line: $key"
    value=${line#*=}
    if [ -z "${!key+set}" ]; then export "$key=${value//\$\{HOME\}/$HOME}"; fi
  done <"$env_file"
fi

export ZWIIM_CLOUD_ADDR=${ZWIIM_CLOUD_ADDR:-127.0.0.1:8443}
state=${ZWIIM_DEV_DIR:-$HOME/.zwiim-dev}
mkdir -p "$state"
chmod 700 "$state"
