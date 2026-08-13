#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
vcp_cli_dir=$(cd "$script_dir/.." && pwd)
package_lock="$vcp_cli_dir/alpine-packages.lock.tsv"

base_url=https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/alpine-minirootfs-3.24.1-aarch64.tar.gz
base_sha256=f55a90f69052c5bd6f92cb09a8f47065970830b194c917a006fb94028e721259
apk_tools_url=https://dl-cdn.alpinelinux.org/alpine/v3.24/main/x86_64/apk-tools-static-3.0.7-r0.apk
apk_tools_sha256=ed1c5e82177844249b7c4ecc2653b78eed096be20496b7fb860a9e165b2e5ce1
expected_tar_sha256=19126a747094e5e0e6a762d0542ee34a34b36893e431ba56e8bd90fd6b58df43
expected_archive_sha256=3bb7949b14b4d926b4080e611375b1eed25d152dbf0439b79d9cf36e186247e7
expected_archive_bytes=26870045

for required_tool in curl sha256sum tar zstd stat; do
  if ! command -v "$required_tool" >/dev/null 2>&1; then
    printf 'Missing build tool: %s\n' "$required_tool" >&2
    exit 2
  fi
done

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/vcp-mobile-rootfs.XXXXXX")
cleanup() {
  case "$work_dir" in
    /tmp/vcp-mobile-rootfs.*|"${TMPDIR:-/tmp}"/vcp-mobile-rootfs.*)
      rm -rf -- "$work_dir"
      ;;
    *)
      printf 'Refusing to clean unexpected path: %s\n' "$work_dir" >&2
      ;;
  esac
}
trap cleanup EXIT

download_checked() {
  local url=$1
  local destination=$2
  local expected=$3
  curl -fL --retry 5 --retry-all-errors --connect-timeout 20 --output "$destination" "$url"
  local actual
  actual=$(sha256sum "$destination" | cut -d' ' -f1)
  if [ "$actual" != "$expected" ]; then
    printf 'SHA-256 mismatch for %s: expected %s, got %s\n' "$url" "$expected" "$actual" >&2
    exit 3
  fi
}

base_archive="$work_dir/alpine-minirootfs.tar.gz"
apk_tools_archive="$work_dir/apk-tools-static.apk"
download_checked "$base_url" "$base_archive" "$base_sha256"
download_checked "$apk_tools_url" "$apk_tools_archive" "$apk_tools_sha256"

apk_tools_dir="$work_dir/apk-tools"
package_dir="$work_dir/packages"
rootfs_dir="$work_dir/rootfs"
mkdir -p "$apk_tools_dir" "$package_dir" "$rootfs_dir"
tar -xzf "$apk_tools_archive" -C "$apk_tools_dir" sbin/apk.static
tar -xzf "$base_archive" -C "$rootfs_dir"

package_files=()
while IFS=$'\t' read -r name version license repository sha256 bytes; do
  if [ -z "$name" ] || [[ "$name" == \#* ]]; then
    continue
  fi
  filename="${name}-${version}.apk"
  destination="$package_dir/$filename"
  url="https://dl-cdn.alpinelinux.org/alpine/v3.24/${repository}/aarch64/${filename}"
  download_checked "$url" "$destination" "$sha256"
  actual_bytes=$(stat -c '%s' "$destination")
  if [ "$actual_bytes" != "$bytes" ]; then
    printf 'Size mismatch for %s: expected %s, got %s\n' "$filename" "$bytes" "$actual_bytes" >&2
    exit 3
  fi
  package_files+=("$destination")
done < "$package_lock"

if [ "${#package_files[@]}" -ne 72 ]; then
  printf 'Package lock drift: expected 72 APKs, got %s\n' "${#package_files[@]}" >&2
  exit 3
fi

"$apk_tools_dir/sbin/apk.static" \
  --root "$rootfs_dir" \
  --no-network \
  --allow-untrusted \
  --no-scripts \
  add "${package_files[@]}"

mkdir -p "$rootfs_dir/workspace" "$rootfs_dir/skills"
# apk.log embeds the random build directory and wall-clock time. The lock file is
# the durable package audit record; omit this generated log so the rootfs bytes
# are reproducible across independent build directories.
rm -f -- "$rootfs_dir/var/log/apk.log"

tar_args=(
  --sort=name
  --mtime=@0
  --owner=0
  --group=0
  --numeric-owner
  --format=posix
  --pax-option=delete=atime,delete=ctime
  --exclude=./var/cache/apk/*
  -C "$rootfs_dir"
  .
)

canonical_tar="$work_dir/rootfs.tar"
tar -cf "$canonical_tar" "${tar_args[@]}"
actual_tar_sha256=$(sha256sum "$canonical_tar" | cut -d' ' -f1)
if [ "$actual_tar_sha256" != "$expected_tar_sha256" ]; then
  printf 'Rootfs tar content drift: expected %s, got %s\n' "$expected_tar_sha256" "$actual_tar_sha256" >&2
  exit 4
fi

candidate="$work_dir/vcp-cli-rootfs.tar.zst"
zstd -q -19 -T1 "$canonical_tar" -o "$candidate"
actual_archive_sha256=$(sha256sum "$candidate" | cut -d' ' -f1)
actual_archive_bytes=$(stat -c '%s' "$candidate")
if [ "$actual_archive_sha256" != "$expected_archive_sha256" ] || [ "$actual_archive_bytes" != "$expected_archive_bytes" ]; then
  printf 'Compressed rootfs drift: sha256=%s bytes=%s; release asset was built with zstd 1.4.8\n' \
    "$actual_archive_sha256" "$actual_archive_bytes" >&2
  exit 4
fi

output=${1:-"$vcp_cli_dir/android-assets/vcp-cli-rootfs-3.24.1-aarch64.tar.zst"}
mkdir -p "$(dirname "$output")"
install -m 0644 "$candidate" "$output"
printf 'Built %s (%s bytes, sha256=%s)\n' "$output" "$actual_archive_bytes" "$actual_archive_sha256"
