# Performance and stability scripts

This directory contains lightweight performance/stability helpers intended for **local development and manual debugging only**. They are **not executed in CI Release builds** and their outputs are **not uploaded as Release assets**.

Current scope:

- APK size report
- Android cold-start timing via `adb shell am start -W`
- Android dumpsys/logcat snapshot collection
- Full-sync pipeline sampling (Mobile PSS/RSS, optional local Node/CDS RSS,
  filtered sync metrics)
- Rust Criterion benchmark artifact capture

Timestamped device captures under `tests/perf/reports/` are local-only and
ignored by Git because they can contain device serials, logcat, and unrelated
system state. Extract only a sanitized aggregate into tracked documentation.
Debug/HMR measurements are diagnostic baselines, not Release acceptance data.

Evidence labels used by the active performance lane:

- `D0`: `tauri android dev` / Vite HMR under the isolated
  `com.vcp.avatar.debug` package; use only for mechanism and DevTools A/B.
- `D1`: production frontend with Android/Rust Debug; use for packaged-debug
  replication, not as signed Release evidence.
- `R1`: same-commit, same-version, signed arm64 Release A/B.

The installed `com.vcp.avatar` package is the user's formal app and must never
be overwritten by a performance experiment. Device experiments must resolve and
verify the `.debug` application ID before install/launch. The current Gradle
debug type uses `applicationIdSuffix = ".debug"`; scripts must still verify the
resolved APK/package identity rather than infer it from the output filename.

Long soak tests are intentionally left as a documented/manual gate for now; they
require a fixed real device, stable power/network conditions, and explicit
approval of thresholds.

## Commands

```bash
# APK size report
node tests/perf/scripts/measure_apk_size.cjs --apk path/to/app.apk --out tests/perf/reports/apk-size.json

# Cold startup samples
node tests/perf/scripts/measure_startup_adb.cjs --samples 10 --out tests/perf/reports/startup.json

# Collect dumpsys/logcat snapshots
node tests/perf/scripts/collect_android_dumpsys.cjs --out-dir tests/perf/reports/manual-run

# Record a full-sync run; start this before tapping Sync and stop it after Final ACK
pnpm perf:sync-pipeline -- --serial <serial> --out-dir tests/perf/reports/full-sync-worker-2

# Add upstream RSS only when VChat Node/CDS run on this same host
pnpm perf:sync-pipeline -- --serial <serial> --node-pid <pid> --cds-pid <pid>

# Compile or run Rust benchmarks
node tests/perf/scripts/run_rust_bench.cjs --no-run --out-dir tests/perf/reports/rust-bench
node tests/perf/scripts/run_rust_bench.cjs --out-dir tests/perf/reports/rust-bench
```

The startup helper resolves the installed package's explicit Launcher activity
before calling `am start -W`. This is required on Android builds where a
package-only MAIN/LAUNCHER intent does not resolve reliably.

The full-sync sampler targets only the already-running `com.vcp.avatar.debug`
process. It does not install, launch, clear, or configure the app. By default it
records `/proc` RSS every 500ms and the heavier `dumpsys` PSS every 2 seconds,
while retaining only a
whitelist of SQLite timing, Flush, and completion log lines. Reports remain under
the ignored `tests/perf/reports/` tree and may include Topic IDs. Set
`syncLogLevel=DEBUG` before the run so normal SQLite writer metrics are present.
The current Rust Pull path no longer emits per-chunk, per-frame, or per-Topic
profiling records. Report schema v2 therefore covers process memory, SQLite writer
metrics, Queue Flush latency, and terminal milestones without presenting missing
Pull observations as zero work. Reintroduce equivalent Debug-only instrumentation
before using this helper to compare NDJSON throughput or Topic preparation time.
The summary records the explicit `FinalAck accepted` marker before the final
Queue Flush, so a network-complete run is not confused with a durable,
acknowledged sync.

Node/CDS memory is not observable through the phone. `--node-pid` and
`--cds-pid` sample `/proc/<pid>/status` only when those processes are on the
same Linux host as this command; otherwise collect upstream memory separately.

`am start -W` ends at Activity display. It does not measure AppLifecycle READY,
chat-shell paint, rich rendering settled, or energy. Current results remain
report-only; there is no frozen CI/Release threshold.

## START_NOT_STICKY note

`StreamKeepaliveService` currently returns `START_NOT_STICKY`, while older project
documentation described `START_STICKY`. Stage four records this difference and
does not change service behavior. `SseProxyService` still uses `START_STICKY`.
