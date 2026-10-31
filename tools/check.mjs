import { execFileSync, spawn } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const backend = fileURLToPath(new URL('../src-tauri/', import.meta.url));
const android = fileURLToPath(new URL('../src-tauri/gen/android/', import.meta.url));
const windows = process.platform === 'win32';
const release = process.argv[2] === '--release';

function run(name, command, args, cwd) {
  return new Promise((resolve) => {
    const started = performance.now();
    console.log(`[check] ${name}: starting`);
    // Windows batch wrappers need cmd.exe. The commands and arguments below are fixed.
    if (windows && /\.(?:cmd|bat)$/i.test(command)) {
      args = ['/d', '/s', '/c', command, ...args];
      command = process.env.ComSpec || 'cmd.exe';
    }
    const child = spawn(command, args, { cwd, stdio: 'inherit', windowsHide: true });
    let failedToStart = false;
    child.once('error', (error) => {
      failedToStart = true;
      console.error(`[check] ${name}: ${error.message}`);
    });
    child.once('close', (code, signal) => {
      const exitCode = failedToStart ? 1 : (code ?? 1);
      const status = exitCode === 0 ? 'passed' : `failed (${signal ?? exitCode})`;
      console.log(`[check] ${name}: ${status} in ${((performance.now() - started) / 1000).toFixed(2)}s`);
      resolve(exitCode);
    });
  });
}

function pnpm(name, args) {
  return run(name, windows ? 'pnpm.cmd' : 'pnpm', args, root);
}

async function checkSources() {
  const contract = await pnpm('config:check', ['config:check']);
  if (contract !== 0) return contract;
  // Validate the contract before its consumers, and avoid competing Cargo jobs.
  const results = await Promise.all([
    pnpm('lint', ['lint']),
    // The production build script already runs vue-tsc before bundling.
    release ? pnpm('production build', ['build']) : pnpm('vue-tsc', [
      'exec', 'vue-tsc', '--noEmit', '--incremental',
      '--tsBuildInfoFile', 'node_modules/.cache/vue-tsc.tsbuildinfo',
    ]),
    pnpm('tsc:node', ['exec', 'tsc', '--noEmit', '--project', 'tsconfig.node.json']),
    run('cargo check', 'cargo', ['check', '--locked'], backend),
  ]);
  // Let every started check finish so diagnostics are complete before exiting.
  return results.find((code) => code !== 0) ?? 0;
}

function checkGeneratedTrees() {
  const changes = execFileSync('git', [
    'ls-files', '--modified', '--others', '--exclude-standard', '--',
    'src-tauri/gen/android', 'src-tauri/plugins/vcp-mobile/permissions',
  ], { cwd: root, encoding: 'utf8', windowsHide: true });
  if (!changes) return 0;
  console.error(`[check] Android project or plugin permissions have unstaged changes. Review before staging:\n${changes.trimEnd()}`);
  return 1;
}

async function checkRelease() {
  const steps = [
    checkGeneratedTrees,
    () => run('rustfmt', 'cargo', ['fmt', '--all', '--', '--check'], backend),
    checkSources,
    () => pnpm('iOS project and archive checks', ['check:ios']),
    () => run('Clippy', 'cargo', ['clippy', '--locked', '--workspace', '--lib', '--tests', '--', '-D', 'warnings'], backend),
    () => pnpm('frontend tests', ['test:run']),
    () => run('Rust unit tests', 'cargo', ['test', '--locked', '--workspace', '--lib'], backend),
    () => pnpm('Rust integration tests', ['test:integration']),
    () => pnpm('Android project', ['tauri', 'android', 'init', '--ci', '--skip-targets-install']),
    () => pnpm('Android settings', ['ci:prepare-android-settings']),
    checkGeneratedTrees,
    () => run('Android plugin tests', windows ? 'gradlew.bat' : './gradlew', [':tauri-plugin-vcp-mobile:testDebugUnitTest'], android),
    () => pnpm('dependency audit', ['audit', '--audit-level=high']),
  ];
  for (const step of steps) {
    const code = await step();
    if (code !== 0) return code;
  }
  return 0;
}

const started = performance.now();
const exitCode = await (release ? checkRelease() : checkSources());
console.log(`[check] total: ${exitCode === 0 ? 'passed' : 'failed'} in ${((performance.now() - started) / 1000).toFixed(2)}s`);
process.exitCode = exitCode;
