# VCP Mobile CLI third-party runtime inventory

## Host executable

| Component | Pinned source | License evidence | Distribution note |
|---|---|---|---|
| Termux PRoot `v5.1.107.89` | `a89b3732ec6ae1db674510f0843b2f3db54d0a2f` | source headers: GPL v2 or later; repository `COPYING`: GPL v2 text | shipped as `libvcp_proot.so` plus its unbundled `libvcp_proot_loader.so`, both under APK `jniLibs`; publish corresponding patched source and build instructions |
| talloc `2.4.2` | SHA-256 `85ecf9…d8a6` | source headers: LGPL v3 or later | statically linked into PRoot; release source/object/relink obligations require explicit review |

The PRoot build carries only two maintenance changes: adding the C standard header required by NDK 29 and
rewriting two GNU-awk-only numeric conversions into POSIX-compatible awk expressions. It does not contain
OpenMinis native-offload code. `PROOT_UNBUNDLE_LOADER` is an upstream build mode, not an additional source
patch; it prevents PRoot from materializing an executable loader into an app-writable runtime directory.

## Alpine guest

- Base: Alpine minirootfs 3.24.1 aarch64, SHA-256 `f55a90f…1259`.
- Package versions, APK byte hashes and SPDX-style license identifiers are fixed in
  [`alpine-packages.lock.tsv`](./alpine-packages.lock.tsv).
- The image is an aggregate of independent programs. Examples include Bash/coreutils/findutils under
  GPL-3.0-or-later, Git/BusyBox/apk-tools under GPL-2.0-family terms, Python under PSF-2.0, OpenSSL under
  Apache-2.0, and musl under MIT.

## Manual terminal UI and PTY host

- `@xterm/xterm 5.5.0` and `@xterm/addon-fit 0.10.0` are distributed under the MIT license; their
  notices must remain in the web dependency license inventory.
- `libvcp_pty.so` is VCPMobile-owned JNI code built from this repository with NDK 29 for arm64-v8a.
  It uses Android/Bionic PTY APIs and does not contain OpenMinis native-offload code.

## Release gate

Before public distribution, release automation must attach or otherwise provide:

1. the exact PRoot patch/build files, both shipped ELF identities and corresponding source archive;
2. talloc source plus whatever relinkable object/source offer the selected legal review requires;
3. Alpine package notices/license texts and corresponding source availability;
4. a notice that Android guest `root` is simulated and grants no Android Root privilege.
5. the xterm.js MIT notices used by the manual terminal renderer.

This inventory records engineering facts; it is not a substitute for a legal review.
