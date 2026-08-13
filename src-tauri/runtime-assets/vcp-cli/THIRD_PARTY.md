# VCP Mobile CLI third-party runtime inventory

## Host executable

| Component | Pinned source | License evidence | Distribution note |
|---|---|---|---|
| Termux PRoot `v5.1.107.89` | `a89b3732ec6ae1db674510f0843b2f3db54d0a2f` | source headers: GPL v2 or later; repository `COPYING`: GPL v2 text | shipped as the separate executable `libvcp_proot.so`; publish corresponding patched source and build instructions |
| talloc `2.4.2` | SHA-256 `85ecf9…d8a6` | source headers: LGPL v3 or later | statically linked into PRoot; release source/object/relink obligations require explicit review |

The PRoot build carries only two maintenance changes: adding the C standard header required by NDK 29 and
rewriting two GNU-awk-only numeric conversions into POSIX-compatible awk expressions. It does not contain
OpenMinis native-offload code.

## Alpine guest

- Base: Alpine minirootfs 3.24.1 aarch64, SHA-256 `f55a90f…1259`.
- Package versions, APK byte hashes and SPDX-style license identifiers are fixed in
  [`alpine-packages.lock.tsv`](./alpine-packages.lock.tsv).
- The image is an aggregate of independent programs. Examples include Bash/coreutils/findutils under
  GPL-3.0-or-later, Git/BusyBox/apk-tools under GPL-2.0-family terms, Python under PSF-2.0, OpenSSL under
  Apache-2.0, and musl under MIT.

## Release gate

Before public distribution, release automation must attach or otherwise provide:

1. the exact PRoot patch/build files and corresponding source archive;
2. talloc source plus whatever relinkable object/source offer the selected legal review requires;
3. Alpine package notices/license texts and corresponding source availability;
4. a notice that Android guest `root` is simulated and grants no Android Root privilege.

This inventory records engineering facts; it is not a substitute for a legal review.
