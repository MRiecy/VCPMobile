#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
vcp_cli_dir=$(cd "$script_dir/.." && pwd)
tauri_dir=$(cd "$vcp_cli_dir/../../.." && pwd)

proot_version=5.1.107.89
proot_url="https://github.com/termux/proot/archive/v${proot_version}.zip"
proot_sha256=e1240f63de03e6da536d74041c7937ddd8737ab27743857d79285724b948eca8
talloc_version=2.4.2
talloc_url="https://download.samba.org/pub/talloc/talloc-${talloc_version}.tar.gz"
talloc_sha256=85ecf9e465e20f98f9950a52e9a411e14320bc555fa257d87697b7e7a9b1d8a6
expected_binary_sha256=651a5778979523e61b534b34a7a649b64f5a43237f951d7d5651b8dcdbe69e86
expected_binary_bytes=281296

android_ndk=${VCP_CLI_ANDROID_NDK:?Set VCP_CLI_ANDROID_NDK to Android NDK 29.0.13846066}
if [ "$(basename "$android_ndk")" != "29.0.13846066" ]; then
  printf 'Unsupported NDK: %s (expected 29.0.13846066)\n' "$android_ndk" >&2
  exit 2
fi

host_tag=linux-x86_64
toolchain_bin="$android_ndk/toolchains/llvm/prebuilt/$host_tag/bin"
cc="$toolchain_bin/aarch64-linux-android26-clang"
ar="$toolchain_bin/llvm-ar"
ranlib="$toolchain_bin/llvm-ranlib"
strip="$toolchain_bin/llvm-strip"
objcopy="$toolchain_bin/llvm-objcopy"
objdump="$toolchain_bin/llvm-objdump"

for required_tool in curl sha256sum unzip tar make patch "$cc" "$ar" "$ranlib" "$strip" "$objcopy" "$objdump"; do
  if ! command -v "$required_tool" >/dev/null 2>&1 && [ ! -x "$required_tool" ]; then
    printf 'Missing build tool: %s\n' "$required_tool" >&2
    exit 2
  fi
done

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/vcp-mobile-proot.XXXXXX")
cleanup() {
  case "$work_dir" in
    /tmp/vcp-mobile-proot.*|"${TMPDIR:-/tmp}"/vcp-mobile-proot.*)
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
  curl -fL --retry 3 --output "$destination" "$url"
  local actual
  actual=$(sha256sum "$destination" | cut -d' ' -f1)
  if [ "$actual" != "$expected" ]; then
    printf 'SHA-256 mismatch for %s: expected %s, got %s\n' "$url" "$expected" "$actual" >&2
    exit 3
  fi
}

proot_archive="$work_dir/proot.zip"
talloc_archive="$work_dir/talloc.tar.gz"
download_checked "$proot_url" "$proot_archive" "$proot_sha256"
download_checked "$talloc_url" "$talloc_archive" "$talloc_sha256"

unzip -q "$proot_archive" -d "$work_dir"
tar -xzf "$talloc_archive" -C "$work_dir"
proot_root="$work_dir/proot-${proot_version}"
talloc_root="$work_dir/talloc-${talloc_version}"

patch -d "$proot_root" -p1 -i "$script_dir/0001-ndk29-c-headers-and-posix-awk.patch"

compat_dir="$work_dir/talloc-compat"
mkdir -p "$compat_dir" "$work_dir/talloc-build"
install -m 0644 "$script_dir/talloc-replace.h" "$compat_dir/replace.h"

"$cc" -c "$talloc_root/talloc.c" \
  -o "$work_dir/talloc-build/talloc.o" \
  -I"$compat_dir" -I"$talloc_root" \
  -DHAVE_STDARG_H=1 -DHAVE_VA_COPY=1 -DHAVE_UNISTD_H=1 -DHAVE_INTPTR_T=1 \
  -fPIC -O2 -Wall -std=gnu99 \
  "-ffile-prefix-map=$work_dir=/usr/src/vcp-mobile-cli"
"$ar" rcs "$work_dir/talloc-build/libtalloc.a" "$work_dir/talloc-build/talloc.o"
"$ranlib" "$work_dir/talloc-build/libtalloc.a"

make -C "$proot_root/src" \
  CC="$cc" \
  STRIP="$strip" \
  OBJCOPY="$objcopy" \
  OBJDUMP="$objdump" \
  CPPFLAGS="-D_FILE_OFFSET_BITS=64 -D_GNU_SOURCE -I. -DARG_MAX=131072 -I$talloc_root -I$compat_dir" \
  CFLAGS="-O2 -Wall -Wextra -fPIE -ffile-prefix-map=$work_dir=/usr/src/vcp-mobile-cli" \
  LDFLAGS="-Wl,-z,noexecstack -pie -L$work_dir/talloc-build -ltalloc" \
  -j"${VCP_CLI_BUILD_JOBS:-4}"

built_binary="$proot_root/src/proot"
"$strip" "$built_binary"
actual_sha256=$(sha256sum "$built_binary" | cut -d' ' -f1)
actual_bytes=$(stat -c '%s' "$built_binary")
if [ "$actual_sha256" != "$expected_binary_sha256" ] || [ "$actual_bytes" != "$expected_binary_bytes" ]; then
  printf 'PRoot output drift: sha256=%s bytes=%s\n' "$actual_sha256" "$actual_bytes" >&2
  exit 4
fi

output=${1:-"$tauri_dir/plugins/vcp-mobile/android/src/main/jniLibs/arm64-v8a/libvcp_proot.so"}
mkdir -p "$(dirname "$output")"
install -m 0755 "$built_binary" "$output"
printf 'Built %s (%s bytes, sha256=%s)\n' "$output" "$actual_bytes" "$actual_sha256"
