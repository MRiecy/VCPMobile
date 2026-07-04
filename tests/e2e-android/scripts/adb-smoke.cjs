const { runAdb, getDeviceInfo, getPackageName } = require('./adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/e2e-android/scripts/adb-smoke.cjs [--mode debug|release]

Launches the app, captures basic activity/logcat state, and exits non-zero only for
ADB-level failures. UI assertions are intentionally light; detailed user journeys are
kept for future Maestro/UIAutomator layers.
`);
}

function parseArgs(argv) {
  const args = { mode: 'debug' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--mode') {
      args.mode = argv[++i];
    } else if (arg === '--help' || arg === '-h') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));
const device = getDeviceInfo();
const pkg = getPackageName(args.mode);
console.log(`[adb-smoke] device=${device.serial} ${device.manufacturer} ${device.model} sdk=${device.sdk} package=${pkg}`);

runAdb(['logcat', '-c'], { allowFailure: true });
runAdb(['shell', 'am', 'force-stop', pkg], { allowFailure: true });
const launch = runAdb(['shell', 'monkey', '-p', pkg, '-c', 'android.intent.category.LAUNCHER', '1']);
console.log(`[adb-smoke] launch:\n${launch.trim()}`);

// Give WebView/Tauri a short window to boot before collecting state.
runAdb(['shell', 'sleep', '5'], { allowFailure: true });

const activity = runAdb(['shell', 'dumpsys', 'activity', 'top'], { allowFailure: true });
const processes = runAdb(['shell', 'ps', '-A'], { allowFailure: true });
const logcat = runAdb(['logcat', '-d', '-v', 'time', '-t', '400'], { allowFailure: true });

console.log('--- dumpsys activity top (trimmed) ---');
console.log(activity.split(/\r?\n/).slice(0, 80).join('\n'));
console.log('--- process matches ---');
console.log(processes.split(/\r?\n/).filter((line) => line.includes(pkg.replace('.debug', ''))).join('\n'));
console.log('--- logcat filtered ---');
console.log(logcat
  .split(/\r?\n/)
  .filter((line) => /VcpMobile|ForegroundGuardian|StreamKeepaliveService|SseProxyService|AndroidRuntime|chromium|Tauri/i.test(line))
  .slice(-120)
  .join('\n'));

console.log('[adb-smoke] done');
