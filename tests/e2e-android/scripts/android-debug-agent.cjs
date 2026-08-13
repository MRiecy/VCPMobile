'use strict';

const fs = require('fs');
const net = require('net');
const os = require('os');
const path = require('path');
const { spawn, spawnSync } = require('child_process');
const {
  ROOT,
  DEBUG_PACKAGE,
  findAdb,
  runAdb,
  runAdbBuffer,
  getDeviceInfo,
  timestamp,
  ensureDir,
} = require('./adb-env.cjs');

const SCHEMA_VERSION = 1;
const VITE_PORT = 1420;
const HMR_PORT = 1421;
const DEFAULT_LOG_LINES = 80;
const MAX_LOG_LINES = 200;
const HEARTBEAT_MS = 30_000;
const ARTIFACT_ROOT = path.join(ROOT, '.agent', 'android-debug');
const STATE_PATH = path.join(ARTIFACT_ROOT, 'dev-state.json');

function usage() {
  console.log(`VCPMobile Android Debug Agent CLI

Usage:
  pnpm android:debug:<command> -- [options]
  pnpm android:debug -- <command> [options]

Commands:
  doctor       Check adb, the selected USB device and local tool prerequisites
  dev          Run foreground USB/HMR development with bounded console output
  status       Print a compact device, WebView, package and tunnel snapshot
  logs         Print PID-scoped Debug app logcat without clearing device logs
  snapshot     Save status + bounded logs, optionally one screenshot
  screenshot   Save exactly one screenshot and print only its path and size
  reload       Relaunch only com.vcp.avatar.debug after tunnel readiness checks
  stop         Ask the active dev supervisor to stop and clean owned tunnels
  install      Verify an APK application id, then install only the Debug package
  grant        Best-effort runtime grants for only the Debug package

Options:
  --serial <id>       Required when more than one adb device is connected
  --json              Machine-readable stdout (dev uses NDJSON events)
  --lines <1..200>    Log line budget; default 80
  --level <v|i|w|e>   Android log priority; default i
  --screenshot        Include one screenshot in snapshot
  --out <path>        Snapshot directory or screenshot file under repo or /tmp
  --name <slug>       Screenshot filename slug
  --apk <path>        APK path for install
  --reset-data        Clear Debug app data after a verified Debug APK install

Safety boundary:
  This CLI never accepts a package override or Release mode. It never clears
  global logcat, never removes all adb reverse mappings, and never manipulates
  com.vcp.avatar.
`);
}

function parseArgs(argv) {
  const tokens = [...argv];
  const command = tokens[0] && !tokens[0].startsWith('-') ? tokens.shift() : 'help';
  const options = {
    serial: '',
    json: false,
    lines: DEFAULT_LOG_LINES,
    level: 'i',
    screenshot: false,
    out: '',
    name: '',
    apk: '',
    resetData: false,
  };

  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token === '--') {
      continue;
    }
    if (token === '--serial') {
      options.serial = requireValue(tokens, ++index, token);
    } else if (token === '--json') {
      options.json = true;
    } else if (token === '--lines') {
      options.lines = Number(requireValue(tokens, ++index, token));
    } else if (token === '--level') {
      options.level = requireValue(tokens, ++index, token).toLowerCase();
    } else if (token === '--screenshot') {
      options.screenshot = true;
    } else if (token === '--out') {
      options.out = requireValue(tokens, ++index, token);
    } else if (token === '--name') {
      options.name = requireValue(tokens, ++index, token);
    } else if (token === '--apk') {
      options.apk = requireValue(tokens, ++index, token);
    } else if (token === '--reset-data') {
      options.resetData = true;
    } else if (token === '--help' || token === '-h') {
      return { command: 'help', options };
    } else {
      throw new Error(`Unknown option: ${token}`);
    }
  }

  if (!Number.isInteger(options.lines) || options.lines < 1 || options.lines > MAX_LOG_LINES) {
    throw new Error(`--lines must be an integer between 1 and ${MAX_LOG_LINES}`);
  }
  if (!['v', 'i', 'w', 'e'].includes(options.level)) {
    throw new Error('--level must be one of v, i, w or e');
  }
  if (options.serial) {
    process.env.ANDROID_SERIAL = options.serial;
  }
  return { command, options };
}

function requireValue(tokens, index, option) {
  const value = tokens[index];
  if (!value || value.startsWith('--')) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

function emit(options, event, message, details = undefined) {
  if (options.json) {
    console.log(JSON.stringify({ event, message, ...(details === undefined ? {} : { details }) }));
    return;
  }
  console.log(`[android-debug:${event}] ${message}`);
}

function printPayload(options, payload) {
  if (options.json) {
    console.log(JSON.stringify(payload));
    return;
  }
  for (const [key, value] of Object.entries(payload)) {
    const rendered = typeof value === 'object' && value !== null ? JSON.stringify(value) : String(value);
    console.log(`${key}=${rendered}`);
  }
}

function stripAnsi(value) {
  return value
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/\r/g, '\n');
}

function resolveOutputPath(candidate, fallback) {
  const resolved = path.resolve(ROOT, candidate || fallback);
  const repoPrefix = `${ROOT}${path.sep}`;
  const tmpPrefix = `${path.resolve(os.tmpdir())}${path.sep}`;
  if (resolved !== ROOT && !resolved.startsWith(repoPrefix) && !resolved.startsWith(tmpPrefix)) {
    throw new Error(`Output path must stay under ${ROOT} or ${os.tmpdir()}`);
  }
  return resolved;
}

function packageManagerName() {
  return process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm';
}

function commandVersion(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: 'utf8', stdio: 'pipe' });
  return result.status === 0 ? (result.stdout || '').trim() : null;
}

function parseReverseMappings(output) {
  return output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const parts = line.split(/\s+/);
      if (parts.length >= 3) {
        return { serial: parts[0], local: parts[1], remote: parts[2] };
      }
      if (parts.length === 2) {
        return { serial: process.env.ANDROID_SERIAL || '', local: parts[0], remote: parts[1] };
      }
      return null;
    })
    .filter(Boolean);
}

function selectedReverseMappings() {
  const serial = getDeviceInfo().serial;
  return parseReverseMappings(runAdb(['reverse', '--list'], { allowFailure: true }))
    .filter((mapping) => !mapping.serial || mapping.serial === serial);
}

function ensureUsbDevice() {
  const device = getDeviceInfo();
  if (device.serial.includes(':')) {
    throw new Error(`USB transport required; selected adb serial looks network-based: ${device.serial}`);
  }
  return device;
}

function parseWmValue(output, label) {
  const override = output.match(new RegExp(`Override ${label}:\\s*([^\\r\\n]+)`));
  const physical = output.match(new RegExp(`Physical ${label}:\\s*([^\\r\\n]+)`));
  return (override || physical || [null, ''])[1].trim();
}

function parsePackageInfo(output) {
  const versionName = output.match(/versionName=([^\s]+)/)?.[1] || null;
  const versionCode = output.match(/versionCode=(\d+)/)?.[1] || null;
  return { versionName, versionCode };
}

function currentWebView() {
  const output = runAdb(['shell', 'dumpsys', 'webviewupdate'], { allowFailure: true });
  const match = output.match(/Current WebView package \(name, version\): \(([^,]+),\s*([^)]+)\)/);
  return match ? { provider: match[1], version: match[2] } : null;
}

function canConnect(port, timeoutMs = 350) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(value);
    };
    socket.setTimeout(timeoutMs);
    socket.once('connect', () => finish(true));
    socket.once('timeout', () => finish(false));
    socket.once('error', () => finish(false));
  });
}

async function collectStatus() {
  const device = getDeviceInfo();
  const packagePath = runAdb(['shell', 'pm', 'path', DEBUG_PACKAGE], { allowFailure: true }).trim();
  const installed = packagePath.startsWith('package:');
  const packageDump = installed
    ? runAdb(['shell', 'dumpsys', 'package', DEBUG_PACKAGE], { allowFailure: true })
    : '';
  const pid = installed
    ? runAdb(['shell', 'pidof', '-s', DEBUG_PACKAGE], { allowFailure: true }).trim() || null
    : null;
  const windowDump = runAdb(['shell', 'dumpsys', 'window', 'windows'], { allowFailure: true });
  const focus = windowDump.split(/\r?\n/).find((line) => line.includes('mCurrentFocus='))?.trim() || null;
  const sizeOutput = runAdb(['shell', 'wm', 'size'], { allowFailure: true });
  const densityOutput = runAdb(['shell', 'wm', 'density'], { allowFailure: true });
  const size = parseWmValue(sizeOutput, 'size');
  const densityText = parseWmValue(densityOutput, 'density');
  const density = Number(densityText || '0') || null;
  const sizeMatch = size.match(/^(\d+)x(\d+)$/);
  const cssViewport = sizeMatch && density
    ? {
        width: Math.round((Number(sizeMatch[1]) * 160) / density),
        height: Math.round((Number(sizeMatch[2]) * 160) / density),
      }
    : null;
  const mappings = selectedReverseMappings();
  const [viteListening, hmrListening] = await Promise.all([
    canConnect(VITE_PORT),
    canConnect(HMR_PORT),
  ]);

  return {
    schema: `vcp.android-debug.status.v${SCHEMA_VERSION}`,
    generatedAt: new Date().toISOString(),
    package: DEBUG_PACKAGE,
    releasePackagePolicy: 'com.vcp.avatar is read-only and never manipulated',
    device: {
      serial: device.serial,
      manufacturer: device.manufacturer,
      model: device.model,
      android: device.release,
      api: device.sdk,
      abi: device.abi,
      transport: device.serial.includes(':') ? 'network' : 'usb',
    },
    display: {
      pixels: size || null,
      density,
      cssViewport,
      fontScale: Number(runAdb(['shell', 'settings', 'get', 'system', 'font_scale'], { allowFailure: true }).trim() || '0') || null,
      navigationMode: runAdb(['shell', 'settings', 'get', 'secure', 'navigation_mode'], { allowFailure: true }).trim() || null,
    },
    webView: currentWebView(),
    app: {
      installed,
      ...parsePackageInfo(packageDump),
      pid,
      foreground: Boolean(focus && focus.includes(DEBUG_PACKAGE)),
      focus,
    },
    dev: {
      host: '127.0.0.1',
      vitePort: VITE_PORT,
      hmrPort: HMR_PORT,
      viteListening,
      hmrListening,
      reverse: mappings,
      ready: viteListening
        && mappings.some((item) => item.local === `tcp:${VITE_PORT}` && item.remote === `tcp:${VITE_PORT}`),
    },
  };
}

function printStatus(options, status) {
  if (options.json) {
    console.log(JSON.stringify(status));
    return;
  }
  const summary = {
    schema: status.schema,
    package: status.package,
    device: `${status.device.serial} ${status.device.manufacturer} ${status.device.model}`,
    android: `${status.device.android} api=${status.device.api} abi=${status.device.abi}`,
    transport: status.device.transport,
    viewport: status.display.cssViewport
      ? `${status.display.cssViewport.width}x${status.display.cssViewport.height} density=${status.display.density} font=${status.display.fontScale}`
      : 'unknown',
    webview: status.webView ? `${status.webView.provider} ${status.webView.version}` : 'unknown',
    app: `installed=${status.app.installed} version=${status.app.versionName || '-'} pid=${status.app.pid || '-'} foreground=${status.app.foreground}`,
    dev: `ready=${status.dev.ready} vite=${status.dev.viteListening} hmr=${status.dev.hmrListening}`,
    reverse: status.dev.reverse,
  };
  printPayload(options, summary);
}

async function commandDoctor(options) {
  const device = ensureUsbDevice();
  const status = await collectStatus();
  const payload = {
    schema: `vcp.android-debug.doctor.v${SCHEMA_VERSION}`,
    ok: true,
    adb: findAdb(),
    pnpm: commandVersion(packageManagerName(), ['--version']),
    node: process.version,
    device: {
      serial: device.serial,
      model: device.model,
      api: device.sdk,
      abi: device.abi,
    },
    debugPackageInstalled: status.app.installed,
    devReady: status.dev.ready,
    artifactRoot: ARTIFACT_ROOT,
  };
  printPayload(options, payload);
  return 0;
}

async function commandStatus(options) {
  printStatus(options, await collectStatus());
  return 0;
}

function collectLogs(options, status) {
  if (!status.app.pid) {
    return {
      schema: `vcp.android-debug.logs.v${SCHEMA_VERSION}`,
      package: DEBUG_PACKAGE,
      pid: null,
      level: options.level,
      limit: options.lines,
      lines: [],
      warning: 'Debug app process is not running; global logcat fallback is intentionally disabled',
    };
  }
  const output = runAdb([
    'logcat',
    '-d',
    `--pid=${status.app.pid}`,
    '-v',
    'threadtime',
    '-t',
    String(options.lines),
    `*:${options.level.toUpperCase()}`,
  ], { allowFailure: true, maxBuffer: 4 * 1024 * 1024 });
  const lines = output.split(/\r?\n/).filter(Boolean).slice(-options.lines);
  return {
    schema: `vcp.android-debug.logs.v${SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    pid: status.app.pid,
    level: options.level,
    limit: options.lines,
    lines,
  };
}

async function commandLogs(options) {
  const result = collectLogs(options, await collectStatus());
  if (options.json) {
    console.log(JSON.stringify(result));
  } else {
    console.log(`package=${result.package} pid=${result.pid || '-'} level=${result.level} lines=${result.lines.length}/${result.limit}`);
    if (result.warning) console.log(`warning=${result.warning}`);
    for (const line of result.lines) console.log(line);
  }
  return result.pid ? 0 : 4;
}

function captureScreenshot(outputPath) {
  const png = runAdbBuffer(['exec-out', 'screencap', '-p']);
  const pngSignature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (png.length < pngSignature.length || !png.subarray(0, pngSignature.length).equals(pngSignature)) {
    throw new Error('adb screencap did not return a PNG payload');
  }
  ensureDir(path.dirname(outputPath));
  fs.writeFileSync(outputPath, png);
  return { path: outputPath, bytes: png.length };
}

function screenshotPath(options) {
  if (options.out) {
    return resolveOutputPath(options.out, '');
  }
  const slug = (options.name || 'screen').replace(/[^a-zA-Z0-9_-]/g, '-').slice(0, 48) || 'screen';
  return path.join(ARTIFACT_ROOT, 'screenshots', `${timestamp()}-${slug}.png`);
}

async function commandScreenshot(options) {
  ensureUsbDevice();
  const result = captureScreenshot(screenshotPath(options));
  printPayload(options, {
    schema: `vcp.android-debug.screenshot.v${SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    ...result,
    imageEmbeddedInStdout: false,
  });
  return 0;
}

async function commandSnapshot(options) {
  ensureUsbDevice();
  const status = await collectStatus();
  const logs = collectLogs(options, status);
  const outDir = resolveOutputPath(
    options.out,
    path.join('.agent', 'android-debug', 'snapshots', timestamp()),
  );
  ensureDir(outDir);
  fs.writeFileSync(path.join(outDir, 'status.json'), `${JSON.stringify(status, null, 2)}\n`, 'utf8');
  fs.writeFileSync(path.join(outDir, 'logcat.txt'), `${logs.lines.join('\n')}\n`, 'utf8');
  let screenshot = null;
  if (options.screenshot) {
    screenshot = captureScreenshot(path.join(outDir, 'screen.png'));
  }
  const manifest = {
    schema: `vcp.android-debug.snapshot.v${SCHEMA_VERSION}`,
    generatedAt: new Date().toISOString(),
    package: DEBUG_PACKAGE,
    files: {
      status: path.join(outDir, 'status.json'),
      logs: path.join(outDir, 'logcat.txt'),
      screenshot: screenshot?.path || null,
    },
    logLines: logs.lines.length,
    screenshotBytes: screenshot?.bytes || 0,
  };
  fs.writeFileSync(path.join(outDir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  printPayload(options, manifest);
  return 0;
}

function resolveLauncherComponent() {
  const output = runAdb([
    'shell',
    'cmd',
    'package',
    'resolve-activity',
    '--brief',
    '-a',
    'android.intent.action.MAIN',
    '-c',
    'android.intent.category.LAUNCHER',
    '--user',
    '0',
    DEBUG_PACKAGE,
  ], { allowFailure: true });
  const component = output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .findLast((line) => line.includes('/'));
  if (!component || !component.startsWith(`${DEBUG_PACKAGE}/`)) {
    throw new Error(`Unable to resolve Debug launcher component for ${DEBUG_PACKAGE}`);
  }
  return component;
}

async function commandReload(options) {
  ensureUsbDevice();
  const before = await collectStatus();
  if (!before.dev.viteListening || !before.dev.ready) {
    throw new Error('USB Dev is not ready; start pnpm android:debug:dev first');
  }
  const component = resolveLauncherComponent();
  runAdb(['shell', 'am', 'force-stop', DEBUG_PACKAGE]);
  runAdb(['shell', 'am', 'start', '-n', component]);
  await new Promise((resolve) => setTimeout(resolve, 1200));
  const after = await collectStatus();
  printPayload(options, {
    schema: `vcp.android-debug.reload.v${SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    component,
    pid: after.app.pid,
    foreground: after.app.foreground,
  });
  return after.app.foreground ? 0 : 5;
}

function setupReversePorts() {
  const existing = selectedReverseMappings();
  const owned = [];
  for (const port of [VITE_PORT, HMR_PORT]) {
    const local = `tcp:${port}`;
    const remote = `tcp:${port}`;
    const current = existing.find((mapping) => mapping.local === local);
    if (current) {
      if (current.remote !== remote) {
        throw new Error(`adb reverse conflict: ${local} already maps to ${current.remote}`);
      }
      continue;
    }
    runAdb(['reverse', local, remote]);
    owned.push(local);
  }
  return owned;
}

function cleanupReversePorts(ownedPorts) {
  for (const local of ownedPorts) {
    runAdb(['reverse', '--remove', local], { allowFailure: true });
  }
}

function writeState(state) {
  ensureDir(path.dirname(STATE_PATH));
  const tempPath = `${STATE_PATH}.tmp`;
  fs.writeFileSync(tempPath, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
  fs.renameSync(tempPath, STATE_PATH);
}

function readState() {
  try {
    return JSON.parse(fs.readFileSync(STATE_PATH, 'utf8'));
  } catch {
    return null;
  }
}

function processAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function isExpectedSupervisor(state) {
  if (!state || !processAlive(state.supervisorPid)) return false;
  if (process.platform !== 'linux') return true;
  try {
    const cmdline = fs.readFileSync(`/proc/${state.supervisorPid}/cmdline`, 'utf8');
    return cmdline.includes('android-debug-agent.cjs') && cmdline.includes('dev');
  } catch {
    return false;
  }
}

function killChildTree(child, signal) {
  if (!child?.pid) return;
  try {
    if (process.platform === 'win32') {
      spawnSync('taskkill', ['/pid', String(child.pid), '/t'], { stdio: 'ignore' });
    } else {
      process.kill(-child.pid, signal);
    }
  } catch {
    // The child may already have exited.
  }
}

function commandDev(options) {
  const device = ensureUsbDevice();
  const previous = readState();
  if (isExpectedSupervisor(previous)) {
    throw new Error(`Android Debug dev is already active under PID ${previous.supervisorPid}`);
  }
  if (previous) {
    fs.rmSync(STATE_PATH, { force: true });
  }

  const ownedPorts = setupReversePorts();
  const logPath = path.join(ARTIFACT_ROOT, 'dev-logs', `${timestamp()}.log`);
  ensureDir(path.dirname(logPath));
  const logStream = fs.createWriteStream(logPath, { flags: 'wx' });
  const environment = {
    ...process.env,
    ANDROID_SERIAL: device.serial,
    TAURI_DEV_HOST: '127.0.0.1',
    NO_COLOR: '1',
    CARGO_TERM_COLOR: 'never',
    GRADLE_OPTS: `${process.env.GRADLE_OPTS || ''} -Dorg.gradle.console=plain`.trim(),
  };
  const child = spawn(
    packageManagerName(),
    ['tauri', 'android', 'dev', '--host', '127.0.0.1'],
    {
      cwd: ROOT,
      env: environment,
      detached: process.platform !== 'win32',
      stdio: ['inherit', 'pipe', 'pipe'],
    },
  );

  const state = {
    schema: `vcp.android-debug.dev-state.v${SCHEMA_VERSION}`,
    supervisorPid: process.pid,
    childPid: child.pid,
    serial: device.serial,
    package: DEBUG_PACKAGE,
    startedAt: new Date().toISOString(),
    logPath,
    ownedReversePorts: ownedPorts,
  };
  writeState(state);
  emit(options, 'start', `serial=${device.serial} package=${DEBUG_PACKAGE}`, { logPath });
  emit(options, 'tunnel', `vite=${VITE_PORT} hmr=${HMR_PORT} owned=${ownedPorts.length}`);

  const tail = [];
  const emitted = new Set();
  let errorCount = 0;
  const consume = (source, chunk) => {
    logStream.write(`[${source}] ${chunk}`);
    const lines = stripAnsi(String(chunk)).split(/\n+/).map((line) => line.trim()).filter(Boolean);
    for (const line of lines) {
      tail.push(line);
      if (tail.length > 60) tail.shift();
      const milestone = /VITE\s+v.*ready|Detected connected device|Finished .*target|Performing Streamed Install|^Success$|Starting: Intent/.test(line);
      const failure = /(^|\b)(error|failed|exception|panic)(:|\b)/i.test(line)
        && !/0 failed|without errors?/i.test(line);
      if (milestone && !emitted.has(line)) {
        emitted.add(line);
        emit(options, 'progress', line);
      } else if (failure && errorCount < 20) {
        errorCount += 1;
        emit(options, 'diagnostic', line);
      }
    }
  };
  child.stdout.on('data', (chunk) => consume('stdout', chunk));
  child.stderr.on('data', (chunk) => consume('stderr', chunk));

  const started = Date.now();
  const heartbeat = setInterval(() => {
    emit(options, 'heartbeat', `running elapsed=${Math.round((Date.now() - started) / 1000)}s log=${logPath}`);
  }, HEARTBEAT_MS);
  heartbeat.unref();

  return new Promise((resolve) => {
    let finished = false;
    const finish = (code, reason, terminateChild) => {
      if (finished) return;
      finished = true;
      clearInterval(heartbeat);
      if (terminateChild) killChildTree(child, reason === 'SIGINT' ? 'SIGINT' : 'SIGTERM');
      cleanupReversePorts(ownedPorts);
      const current = readState();
      if (current?.supervisorPid === process.pid) fs.rmSync(STATE_PATH, { force: true });
      if (code !== 0) {
        emit(options, 'failure-tail', `last ${Math.min(tail.length, 20)} lines are in ${logPath}`, tail.slice(-20));
      }
      emit(options, 'exit', `code=${code} reason=${reason} log=${logPath}`);
      logStream.end();
      resolve(code);
    };

    child.once('error', (error) => {
      consume('spawn-error', error.message);
      finish(1, 'spawn-error', false);
    });
    child.once('exit', (code, signal) => finish(code ?? 1, signal || 'child-exit', false));
    process.once('SIGINT', () => finish(130, 'SIGINT', true));
    process.once('SIGTERM', () => finish(143, 'SIGTERM', true));
  });
}

function commandStop(options) {
  const state = readState();
  if (!state) {
    emit(options, 'stop', 'no active dev state');
    return 0;
  }
  if (!isExpectedSupervisor(state)) {
    fs.rmSync(STATE_PATH, { force: true });
    emit(options, 'stop', 'removed stale state; reverse mappings were left untouched for safety');
    return 0;
  }
  process.kill(state.supervisorPid, 'SIGTERM');
  emit(options, 'stop', `requested supervisor PID ${state.supervisorPid} to stop`);
  return 0;
}

function sdkRoots() {
  return [
    process.env.ANDROID_HOME,
    process.env.ANDROID_SDK_ROOT,
    process.platform === 'win32' && process.env.LOCALAPPDATA
      ? path.join(process.env.LOCALAPPDATA, 'Android', 'Sdk')
      : null,
    path.join(os.homedir(), 'Android', 'Sdk'),
    path.join(os.homedir(), 'Library', 'Android', 'sdk'),
  ].filter(Boolean);
}

function findAapt() {
  const executable = process.platform === 'win32' ? 'aapt.exe' : 'aapt';
  for (const sdk of sdkRoots()) {
    const buildTools = path.join(sdk, 'build-tools');
    if (!fs.existsSync(buildTools)) continue;
    const versions = fs.readdirSync(buildTools).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
    for (const version of versions) {
      const candidate = path.join(buildTools, version, executable);
      if (fs.existsSync(candidate)) return candidate;
    }
  }
  throw new Error('aapt was not found; APK identity verification fails closed');
}

function verifyDebugApk(apkPath) {
  const aapt = findAapt();
  const result = spawnSync(aapt, ['dump', 'badging', apkPath], {
    encoding: 'utf8',
    stdio: 'pipe',
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`aapt could not inspect APK: ${(result.stderr || '').trim()}`);
  }
  const applicationId = (result.stdout || '').match(/^package:\s+name='([^']+)'/m)?.[1] || null;
  if (applicationId !== DEBUG_PACKAGE) {
    throw new Error(`Refusing APK application id ${applicationId || 'unknown'}; expected ${DEBUG_PACKAGE}`);
  }
  return { aapt, applicationId };
}

async function commandInstall(options) {
  ensureUsbDevice();
  if (!options.apk) throw new Error('install requires --apk <path>');
  const apkPath = resolveOutputPath(options.apk, '');
  if (!fs.existsSync(apkPath) || !fs.statSync(apkPath).isFile()) {
    throw new Error(`APK does not exist: ${apkPath}`);
  }
  const verified = verifyDebugApk(apkPath);
  emit(options, 'install', `verified ${verified.applicationId} via ${verified.aapt}`);
  runAdb(['install', '-r', '-d', apkPath], { stdio: 'inherit' });
  if (options.resetData) {
    runAdb(['shell', 'pm', 'clear', DEBUG_PACKAGE]);
  }
  const status = await collectStatus();
  printPayload(options, {
    schema: `vcp.android-debug.install.v${SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    apk: apkPath,
    versionName: status.app.versionName,
    resetData: options.resetData,
  });
  return 0;
}

function commandGrant(options) {
  const device = ensureUsbDevice();
  const permissions = [
    'android.permission.CAMERA',
    'android.permission.RECORD_AUDIO',
    'android.permission.ACCESS_FINE_LOCATION',
    'android.permission.ACCESS_COARSE_LOCATION',
    ...(device.sdk >= 33
      ? ['android.permission.POST_NOTIFICATIONS', 'android.permission.READ_MEDIA_IMAGES']
      : ['android.permission.READ_EXTERNAL_STORAGE']),
  ];
  for (const permission of permissions) {
    runAdb(['shell', 'pm', 'grant', DEBUG_PACKAGE, permission], { allowFailure: true });
  }
  runAdb(['shell', 'dumpsys', 'deviceidle', 'whitelist', `+${DEBUG_PACKAGE}`], { allowFailure: true });
  printPayload(options, {
    schema: `vcp.android-debug.grant.v${SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    attempted: permissions,
    manualOnly: ['notification-listener', 'oem-auto-start', 'battery-unrestricted', 'recents-lock'],
  });
  return 0;
}

async function main(argv = process.argv.slice(2)) {
  const { command, options } = parseArgs(argv);
  if (command === 'help') {
    usage();
    return 0;
  }
  if (command === 'doctor') return commandDoctor(options);
  if (command === 'dev') return commandDev(options);
  if (command === 'status') return commandStatus(options);
  if (command === 'logs') return commandLogs(options);
  if (command === 'snapshot') return commandSnapshot(options);
  if (command === 'screenshot') return commandScreenshot(options);
  if (command === 'reload') return commandReload(options);
  if (command === 'stop') return commandStop(options);
  if (command === 'install') return commandInstall(options);
  if (command === 'grant') return commandGrant(options);
  throw new Error(`Unknown command: ${command}`);
}

if (require.main === module) {
  Promise.resolve(main())
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error) => {
      const json = process.argv.includes('--json');
      if (json) {
        console.error(JSON.stringify({
          event: 'error',
          message: error instanceof Error ? error.message : String(error),
        }));
      } else {
        console.error(`[android-debug:error] ${error instanceof Error ? error.message : String(error)}`);
      }
      process.exitCode = 1;
    });
}

module.exports = { main };
