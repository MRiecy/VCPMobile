import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const backend = fileURLToPath(new URL('../src-tauri/', import.meta.url));
const windows = process.platform === 'win32';

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, stdio: 'inherit', windowsHide: true });
  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function pnpm(args) {
  // Windows' package-manager shim is a batch file. Arguments here are fixed
  // check steps, never interpolated from settings or imported configuration.
  if (windows) run(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', 'pnpm.cmd', ...args], root);
  else run('pnpm', args, root);
}

pnpm(['config:check']);
pnpm(['lint']);
pnpm(['exec', 'vue-tsc', '--noEmit']);
pnpm(['exec', 'tsc', '--noEmit', '--project', 'tsconfig.node.json']);
run('cargo', ['check', '--locked'], backend);
