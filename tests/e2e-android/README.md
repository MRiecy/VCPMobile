# Android E2E smoke (adb-only)

This directory contains lightweight Android smoke/E2E helpers for VCPMobile.

Current scope:

- adb device discovery
- APK install / clean install
- best-effort permission grants
- launch smoke with logcat/activity/process collection

Maestro is intentionally not introduced in the first implementation batch. The
current scripts are stable primitives that can later be wrapped by Maestro or
UIAutomator flows.

## Package names

- Debug: `com.vcp.avatar.debug`
- Release: `com.vcp.avatar`

Override with `E2E_PACKAGE` if needed.

## Common commands

```bash
# Check adb and connected device
node tests/e2e-android/scripts/adb-env.cjs

# Install debug APK cleanly
node tests/e2e-android/scripts/install-apk.cjs --apk src-tauri/gen/android/app/build/outputs/apk/debug/app-debug.apk --mode debug --clean

# Best-effort permission grants
node tests/e2e-android/scripts/grant-permissions.cjs --mode debug

# Launch smoke and collect trimmed state
node tests/e2e-android/scripts/adb-smoke.cjs --mode debug
```

## Manual-only / OEM-dependent items

These cannot be reliably automated with plain adb on all devices:

- notification listener permission
- OEM auto-start permission
- OEM battery unrestricted mode
- recents-task lock
- some overlay permission variants

Record these as device preconditions for P1/P2 tests.
