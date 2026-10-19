# Shared voice MP3 core

`vcp-voice-mp3` converts raw little-endian PCM16 mono at 16 kHz to MPEG-2 Layer III
MP3 at 16 kHz, 64 kbps CBR, LAME quality 5. It accepts 1–480,000 samples (30 seconds).
It does not parse WAV, record audio, call ASR, access files, or select application
settings. Android and iOS use the same Rust/C path; host builds exist for development
and tests only.

Call `encode_pcm16_mono(pcm, is_cancelled)` on a blocking worker. `EncodeError`
implements `Display` and includes `Cancelled`. Cancellation is checked while waiting
for the encoder lock (2 ms sleep between attempts), before initialization, between
2048-sample blocks, and before/after final flush. Each C call must return before
cancellation can be observed. No partial output is returned on cancellation.

The cancellation callback must be short and must not call the encoder recursively.
All native encoder lifetimes are serialized by a private mutex: upstream
`quantize_pvt.c::iteration_init` rewrites global quantization tables for each new
handle, and `util.c::init_log_table` uses a non-atomic static initialization flag.
Protecting only initialization would still race an existing encoder's reads. The
RAII handle closes before the mutex is released on success, failure or unwinding.
No native handle or borrowed PCM pointer escapes a call.

The Rust wrapper converts PCM with `i16::from_le_bytes`, so alignment and host byte
order cannot corrupt input. Chunk and flush buffers use LAME's documented
`1.25 * samples + 7200` bound. The output vector reserves that bound once for the
validated input (at most 607,200 bytes) and rejects overflow; input PCM is borrowed,
not copied wholesale. Native buffers remain limited by the fixed format, block size
and duration. Final `lame_encode_flush` preserves delayed samples and complete frames.

## Source and license

The vendored sources are LAME 4.0 from the official release archive:

- Source: <https://downloads.sourceforge.net/project/lame/lame/4.0/lame-4.0.tar.gz>
- Project: <https://lame.sourceforge.io/>
- Archive SHA-256: `3df5124d5ad3a98312ffd7ba6a9b36230e4f8a3e66d3ce0f425e336c32d216eb`
- License: GNU Library General Public License version 2 or later, preserved in
  `vendor/lame/COPYING`; upstream usage notice is `vendor/lame/LICENSE`. Source file
  copyright and license notices remain intact.

Files were copied directly from the verified archive. There are no edits to upstream
algorithm sources. `native/config.h`, `build.rs` and the Rust wrapper are local
integration files. A release that statically links this code must retain the LAME
notices and provide the source and relinking materials required by its license.
This crate does not perform a release or change the application's release process.

## Exact trimming and build boundary

The vendor tree contains the public `lame.h`, the top-level encoder `.c`/`.h` files
except `mpglib_interface.c`, and the declaration-only `vector/lame_intrin.h` included
by upstream `fft.c`. The explicit build list compiles 19 encoder translation units.
The frontend/CLI, decoder (`mpglib` and mpg123), examples, platform wrappers, assembly,
vector implementations, generated host configuration and third-party media frameworks
are not vendored or compiled. Analyzer hooks are disabled with `NOANALYSIS`.

`gain_analysis.c`, `id3tag.c`, `VbrTag.c` and VBR quantization helpers are retained
because upstream encoder code references them. ReplayGain, automatic ID3 writing and
VBR/LAME tag writing are explicitly disabled for every handle. This is an encoder
core source subset, not a fork that physically removes every optional encoder branch.
Function/data sections allow the final linker to discard unused sections where its
normal target configuration supports that; exact binary savings require target build
measurements.

`cc` follows Cargo's target compiler, sysroot, deployment settings and optimization
level. Mobile targets are ARM64 Android and iOS; portable upstream C is used on both
and on the host. Fixed-width integers come from `stdint.h`, with compile-time checks
for the short/int/float/double ABI. No host `SIZEOF_LONG` assumption is copied into
Apple/Android builds. MSVC uses precise floating-point behavior; clang/GCC receive
`-fno-fast-math` and `-fno-strict-aliasing`. Upstream's normal float/double precision
and fast-log implementation are preserved; x86 SIMD and the optional IEEE754
quantization conversion hack are disabled.

## Validation scope

`cargo test --locked -p vcp-voice-mp3` from `src-tauri/` exercises actual encoding:
every frame's MPEG version, layer, bitrate, rate and channel mode, final frame
completeness, minimum/maximum duration, cancellation during encoding/lock contention,
and concurrent callers. These host tests are not Android or Apple build evidence,
device audio quality evidence, or ASR accuracy measurements. Mobile link/build
validation belongs to the application's normal Android and staged Apple build chains.
