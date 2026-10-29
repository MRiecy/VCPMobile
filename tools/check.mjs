import { spawn } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const backend = fileURLToPath(new URL('../src-tauri/', import.meta.url));
const windows = process.platform === 'win32';

function run(name, command, args, cwd) {
  return new Promise((resolve) => {
    const started = performance.now();
    console.log(`[check] ${name}: starting`);
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
  // Windows' package-manager shim is a batch file. Arguments here are fixed
  // check steps, never interpolated from settings or imported configuration.
  if (windows) return run(name, process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', 'pnpm.cmd', ...args], root);
  return run(name, 'pnpm', args, root);
}

const started = performance.now();
const contractExitCode = await pnpm('config:check', ['config:check']);
let exitCode = contractExitCode;
if (contractExitCode === 0) {
  // Validate the contract before its consumers, and avoid competing Cargo jobs.
  const results = await Promise.all([
    pnpm('lint', ['lint']),
    pnpm('vue-tsc', [
      'exec', 'vue-tsc', '--noEmit', '--incremental',
      '--tsBuildInfoFile', 'node_modules/.cache/vue-tsc.tsbuildinfo',
    ]),
    pnpm('tsc:node', ['exec', 'tsc', '--noEmit', '--project', 'tsconfig.node.json']),
    run('cargo check', 'cargo', ['check', '--locked'], backend),
  ]);
  // Let every started check finish so diagnostics are complete before exiting.
  exitCode = results.find((code) => code !== 0) ?? 0;
}
console.log(`[check] total: ${exitCode === 0 ? 'passed' : 'failed'} in ${((performance.now() - started) / 1000).toFixed(2)}s`);
process.exitCode = exitCode;
