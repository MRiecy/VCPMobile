const fs = require('fs');
const path = require('path');
const { ROOT, runAdb, getPackageName, getDeviceInfo } = require('./adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/e2e-android/scripts/install-apk.cjs --apk <path> [--mode debug|release] [--clean]

Env:
  ANDROID_SERIAL  target device serial when multiple devices are connected
  E2E_PACKAGE     override package name
`);
}

function parseArgs(argv) {
  const args = { mode: 'debug', clean: false, apk: '' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--apk') {
      args.apk = argv[++i];
    } else if (arg === '--mode') {
      args.mode = argv[++i];
    } else if (arg === '--clean') {
      args.clean = true;
    } else if (arg === '--help' || arg === '-h') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  if (!args.apk) {
    throw new Error('缺少 --apk <path>');
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));
const apkPath = path.resolve(ROOT, args.apk);
if (!fs.existsSync(apkPath)) {
  throw new Error(`APK 不存在: ${apkPath}`);
}

const pkg = getPackageName(args.mode);
const device = getDeviceInfo();
console.log(`[install-apk] device=${device.serial} ${device.manufacturer} ${device.model} sdk=${device.sdk}`);
console.log(`[install-apk] package=${pkg} apk=${apkPath}`);

if (args.clean) {
  console.log(`[install-apk] uninstall ${pkg}`);
  runAdb(['uninstall', pkg], { allowFailure: true });
}

runAdb(['install', '-r', '-d', apkPath], { stdio: 'inherit' });

if (args.clean) {
  console.log(`[install-apk] pm clear ${pkg}`);
  runAdb(['shell', 'pm', 'clear', pkg], { allowFailure: true });
}

console.log('[install-apk] done');
