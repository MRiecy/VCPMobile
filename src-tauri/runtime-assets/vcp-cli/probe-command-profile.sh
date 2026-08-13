#!/bin/bash
set -eu

required_commands=(
  bash sh pwd ls mkdir cp mv rm ln touch cat head tail wc stat
  grep sed awk sort uniq cut tr xargs find diff patch
  tar gzip xz zip unzip file jq
  curl wget git ssh scp
  python3 pip apk
)

if [ "${BASH:-}" != "/bin/bash" ]; then
  printf 'PROFILE_ERROR expected=/bin/bash actual=%s\n' "${BASH:-missing}" >&2
  exit 2
fi

missing=0
for command_name in "${required_commands[@]}"; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'PROFILE_MISSING %s\n' "$command_name" >&2
    missing=1
  fi
done

if [ "$missing" -ne 0 ]; then
  exit 3
fi

printf 'PROFILE_OK shell=%s bash=%s alpine=%s arch=%s commands=%s\n' \
  "$BASH" "$BASH_VERSION" "$(cat /etc/alpine-release)" "$(uname -m)" "${#required_commands[@]}"
