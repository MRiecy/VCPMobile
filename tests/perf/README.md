# Performance and stability scripts

This directory contains lightweight performance/stability helpers intended for **local development and manual debugging only**. They are **not executed in CI Release builds** and their outputs are **not uploaded as Release assets**.

Current scope:

- APK size report
- Android cold-start timing via `adb shell am start -W`
- Android dumpsys/logcat snapshot collection
- Rust Criterion benchmark artifact capture

Timestamped device captures under `tests/perf/reports/` are local-only and
ignored by Git because they can contain device serials, logcat, and unrelated
system state. Extract only a sanitized aggregate into tracked documentation.
Debug/HMR measurements are diagnostic baselines, not Release acceptance data.

Long soak tests are intentionally left as a documented/manual gate for now; they
require a fixed real device, stable power/network conditions, and explicit
approval of thresholds.

## Commands

```bash
# APK size report
node tests/perf/scripts/measure_apk_size.cjs --apk path/to/app.apk --out tests/perf/reports/apk-size.json

# Cold startup samples
node tests/perf/scripts/measure_startup_adb.cjs --mode debug --samples 10 --out tests/perf/reports/startup.json

# Collect dumpsys/logcat snapshots
node tests/perf/scripts/collect_android_dumpsys.cjs --mode debug --out-dir tests/perf/reports/manual-run

# Compile or run Rust benchmarks
node tests/perf/scripts/run_rust_bench.cjs --no-run --out-dir tests/perf/reports/rust-bench
node tests/perf/scripts/run_rust_bench.cjs --out-dir tests/perf/reports/rust-bench
```

The startup helper resolves the installed package's explicit Launcher activity
before calling `am start -W`. This is required on Android builds where a
package-only MAIN/LAUNCHER intent does not resolve reliably.

## START_NOT_STICKY note

`StreamKeepaliveService` currently returns `START_NOT_STICKY`, while older project
documentation described `START_STICKY`. Stage four records this difference and
does not change service behavior. `SseProxyService` still uses `START_STICKY`.
