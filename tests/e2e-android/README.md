# Android Debug Agent CLI (adb-only)

This directory contains the tracked, clean-clone-safe Android Debug tool used by
developers and coding agents. The authoritative contract is
[`docs/ANDROID_AGENT_DEBUGGING.md`](../../docs/ANDROID_AGENT_DEBUGGING.md).

The tool has one immutable package boundary:

- Debug: `com.vcp.avatar.debug`
- Release: `com.vcp.avatar` is user-owned and never manipulated

It also has one recommended transport: USB with adb reverse for ports 1420 and
1421. WiFi/TUN auto-detection is intentionally not part of the Agent contract.

## Stable commands

```bash
pnpm android:debug:doctor -- --json
pnpm android:debug:dev -- --serial <adb-serial>
pnpm android:debug:status -- --json
pnpm android:debug:logs -- --lines 80 --level i
pnpm android:debug:snapshot -- --screenshot
pnpm android:debug:screenshot -- --name theme-page
pnpm android:debug:reload
pnpm android:debug:stop
```

`dev` is foreground and low-noise. Complete build output is written under
`.agent/android-debug/dev-logs/`; stdout contains only milestones, bounded
diagnostics and 30-second heartbeats. `logs` is PID-scoped and capped at 200
lines. `snapshot` writes files and prints paths instead of embedding images or
large diagnostics in the caller response.

The retired `adb-smoke.cjs`, `install-apk.cjs` and `grant-permissions.cjs`
entrypoints must not be restored. Their safe Debug-only behavior is available
as `snapshot`, `install` and `grant` commands of the unified CLI.

## Manual-only / OEM-dependent items

These cannot be reliably automated with plain adb on all devices:

- notification listener permission
- OEM auto-start permission
- OEM battery unrestricted mode
- recents-task lock
- legacy overlay variants are not granted; the dormant floating-assistant permission is absent from production manifests

Record these as device preconditions for P1/P2 tests. Maestro/UIAutomator are
still outside this helper's scope; real UI journeys remain explicit acceptance
work rather than an inferred smoke result.
