const fs = require('fs');
const path = require('path');
const { runAdb, getDeviceInfo, getPackageName, ROOT, timestamp, ensureDir } = require('../../e2e-android/scripts/adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/perf/scripts/collect_android_dumpsys.cjs [--mode debug|release] [--out-dir <dir>]

Collects meminfo/power/wifi/dropbox/logcat snapshots for Android performance or
soak-test diagnostics.
`);
}

function parseArgs(argv) {
  const args = { mode: 'debug', outDir: '' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--mode') {
      args.mode = argv[++i];
    } else if (arg === '--out-dir') {
      args.outDir = argv[++i];
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
const outDir = ensureDir(path.resolve(ROOT, args.outDir || path.join('tests', 'perf', 'reports', timestamp())));

const files = {
  'device.json': JSON.stringify({ generated_at: new Date().toISOString(), package: pkg, device }, null, 2),
  'meminfo.txt': runAdb(['shell', 'dumpsys', 'meminfo', pkg], { allowFailure: true, maxBuffer: 32 * 1024 * 1024 }),
  'power.txt': runAdb(['shell', 'dumpsys', 'power'], { allowFailure: true, maxBuffer: 32 * 1024 * 1024 }),
  'wifi.txt': runAdb(['shell', 'dumpsys', 'wifi'], { allowFailure: true, maxBuffer: 32 * 1024 * 1024 }),
  'dropbox.txt': runAdb(['shell', 'dumpsys', 'dropbox', '--print'], { allowFailure: true, maxBuffer: 32 * 1024 * 1024 }),
  'activity-top.txt': runAdb(['shell', 'dumpsys', 'activity', 'top'], { allowFailure: true, maxBuffer: 16 * 1024 * 1024 }),
  'logcat-tail.txt': runAdb(['logcat', '-d', '-v', 'time', '-t', '1000'], { allowFailure: true, maxBuffer: 32 * 1024 * 1024 }),
};

for (const [name, content] of Object.entries(files)) {
  fs.writeFileSync(path.join(outDir, name), content, 'utf8');
}

console.log(`[collect_android_dumpsys] wrote ${outDir}`);
