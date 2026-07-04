const { runAdb, getDeviceInfo, getPackageName } = require('./adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/e2e-android/scripts/grant-permissions.cjs [--mode debug|release]

Attempts best-effort adb grants/appops. Some OEM permissions (notification listener,
auto-start, battery unrestricted, recents lock) still require manual setup.
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

function grant(pkg, permission) {
  console.log(`[grant] ${permission}`);
  runAdb(['shell', 'pm', 'grant', pkg, permission], { allowFailure: true });
}

function appops(pkg, op, mode) {
  console.log(`[appops] ${op} ${mode}`);
  runAdb(['shell', 'appops', 'set', pkg, op, mode], { allowFailure: true });
}

const args = parseArgs(process.argv.slice(2));
const device = getDeviceInfo();
const pkg = getPackageName(args.mode);
console.log(`[grant-permissions] device=${device.serial} sdk=${device.sdk} package=${pkg}`);

grant(pkg, 'android.permission.CAMERA');
grant(pkg, 'android.permission.RECORD_AUDIO');
grant(pkg, 'android.permission.ACCESS_FINE_LOCATION');
grant(pkg, 'android.permission.ACCESS_COARSE_LOCATION');

if (device.sdk >= 33) {
  grant(pkg, 'android.permission.POST_NOTIFICATIONS');
  grant(pkg, 'android.permission.READ_MEDIA_IMAGES');
} else {
  grant(pkg, 'android.permission.READ_EXTERNAL_STORAGE');
}

appops(pkg, 'SYSTEM_ALERT_WINDOW', 'allow');
runAdb(['shell', 'dumpsys', 'deviceidle', 'whitelist', `+${pkg}`], { allowFailure: true });

console.log('[grant-permissions] best-effort grants finished');
console.log('[grant-permissions] Manual-only items may remain: notification listener, OEM auto-start, battery unrestricted, recents lock.');
