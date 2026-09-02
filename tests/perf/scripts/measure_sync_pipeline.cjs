const fs = require("fs");
const path = require("path");
const { execFile, spawn } = require("child_process");
const { promisify } = require("util");
const {
  DEBUG_PACKAGE,
  ROOT,
  ensureDir,
  findAdb,
  getDeviceInfo,
  runAdb,
  timestamp,
} = require("../../e2e-android/scripts/adb-env.cjs");

const execFileAsync = promisify(execFile);
const DEFAULT_INTERVAL_MS = 500;
const DEFAULT_PSS_INTERVAL_MS = 2000;
const DEFAULT_DURATION_SECONDS = 60 * 60;
const MAX_CAPTURE_BYTES = 8 * 1024 * 1024;

function usage() {
  console.log(`Usage:
  node tests/perf/scripts/measure_sync_pipeline.cjs [options]

Continuously samples the running VCPMobile Debug process and records only a
whitelist of sync-pipeline log lines. Stop with Ctrl+C; a summary is written
before exit.

Options:
  --serial <serial>            adb device serial
  --interval-ms <ms>           RSS sample interval (default: 500, min: 250)
  --pss-interval-ms <ms>       dumpsys PSS interval (default: 2000, min: RSS interval)
  --duration-seconds <seconds> maximum capture time (default: 3600)
  --out-dir <dir>              report directory
  --node-pid <pid>             optional local VChat Node process
  --cds-pid <pid>              optional local CDS process
  --help                       show this help
`);
}

function parsePositiveInteger(value, name) {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} 必须是正整数`);
  }
  return parsed;
}

function parseArgs(argv) {
  const args = {
    serial: "",
    intervalMs: DEFAULT_INTERVAL_MS,
    pssIntervalMs: DEFAULT_PSS_INTERVAL_MS,
    durationSeconds: DEFAULT_DURATION_SECONDS,
    outDir: "",
    nodePid: null,
    cdsPid: null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--") {
      continue;
    } else if (arg === "--serial") {
      args.serial = argv[++index] || "";
    } else if (arg === "--interval-ms") {
      args.intervalMs = parsePositiveInteger(argv[++index], "--interval-ms");
    } else if (arg === "--pss-interval-ms") {
      args.pssIntervalMs = parsePositiveInteger(
        argv[++index],
        "--pss-interval-ms",
      );
    } else if (arg === "--duration-seconds") {
      args.durationSeconds = parsePositiveInteger(
        argv[++index],
        "--duration-seconds",
      );
    } else if (arg === "--out-dir") {
      args.outDir = argv[++index] || "";
    } else if (arg === "--node-pid") {
      args.nodePid = parsePositiveInteger(argv[++index], "--node-pid");
    } else if (arg === "--cds-pid") {
      args.cdsPid = parsePositiveInteger(argv[++index], "--cds-pid");
    } else if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  if (args.intervalMs < 250) {
    throw new Error("--interval-ms 不能低于 250ms，避免采样器干扰同步");
  }
  if (args.pssIntervalMs < args.intervalMs) {
    throw new Error("--pss-interval-ms 不能低于 --interval-ms");
  }
  if (!args.serial && process.env.ANDROID_SERIAL) {
    args.serial = process.env.ANDROID_SERIAL;
  }
  return args;
}

function parseProcStatus(text) {
  const values = {};
  for (const line of text.split(/\r?\n/)) {
    const match = line.match(/^([A-Za-z][A-Za-z0-9_]*):\s+(\d+)\s*kB$/);
    if (match) {
      values[match[1]] = Number(match[2]);
    }
  }
  return {
    rssKb: values.VmRSS ?? null,
    hwmKb: values.VmHWM ?? null,
    rssAnonKb: values.RssAnon ?? null,
    rssFileKb: values.RssFile ?? null,
    rssShmemKb: values.RssShmem ?? null,
    swapKb: values.VmSwap ?? null,
  };
}

function parseCheckinPss(text, expectedPid) {
  const line = text
    .split(/\r?\n/)
    .map((item) => item.trim())
    .find((item) => item.startsWith("4,"));
  if (!line) {
    return null;
  }
  const fields = line.split(",");
  if (Number(fields[1]) !== expectedPid || fields.length < 19) {
    return null;
  }
  // ActivityThread checkin v4 fields 16-19 are native, Dalvik, other, and total PSS.
  const totalPssKb = Number(fields[18]);
  return Number.isFinite(totalPssKb) ? totalPssKb : null;
}

function parseRustDurationMs(value) {
  const match = String(value).match(/^([0-9.]+)(ns|µs|us|ms|s)$/);
  if (!match) {
    return null;
  }
  const amount = Number(match[1]);
  const scale = { ns: 1e-6, µs: 1e-3, us: 1e-3, ms: 1, s: 1000 }[match[2]];
  return Number.isFinite(amount) ? amount * scale : null;
}

function percentile(values, ratio) {
  if (values.length === 0) {
    return null;
  }
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil(ratio * sorted.length) - 1];
}

function numericSummary(values) {
  const valid = values.filter(Number.isFinite);
  if (valid.length === 0) {
    return { count: 0, min: null, p50: null, p95: null, max: null, mean: null };
  }
  const total = valid.reduce((sum, value) => sum + value, 0);
  return {
    count: valid.length,
    min: Math.min(...valid),
    p50: percentile(valid, 0.5),
    p95: percentile(valid, 0.95),
    max: Math.max(...valid),
    mean: total / valid.length,
  };
}

function readHostProcess(pid) {
  if (!pid || process.platform !== "linux") {
    return null;
  }
  try {
    return {
      pid,
      ...parseProcStatus(fs.readFileSync(`/proc/${pid}/status`, "utf8")),
    };
  } catch {
    return { pid, unavailable: true };
  }
}

function lineIsAllowed(line) {
  return [
    /\[PullExecutor\] \[(?:ProfileDetail|ProfileSummary|NdjsonFrame|NdjsonStream)\]/,
    /\[Sync\] \[db_write\] operation=/,
    /\[DbWriteQueue\].*Flush/,
    /\[SyncService\] \[FinalAck\] accepted/,
    /\[Sync\] \[(?:messages|sync)\].*(?:Phase 3|Session ended|同步已完成)/,
    /Final sync write drain failed/,
    /final acknowledgement/i,
  ].some((pattern) => pattern.test(line));
}

function updateLogMetrics(line, metrics, receivedAt) {
  if (/Phase 3 skipped/.test(line)) {
    metrics.milestones.phase3SkippedAt ??= receivedAt;
  } else if (/=== Phase 3: Messages ===/.test(line)) {
    metrics.milestones.phase3StartedAt ??= receivedAt;
  }
  if (/Phase 3 completed/.test(line)) {
    metrics.milestones.phase3CompletedAt ??= receivedAt;
  }
  if (/\[FinalAck\] accepted/.test(line)) {
    metrics.milestones.finalAckAcceptedAt ??= receivedAt;
  }
  if (/同步已完成，所有数据已对齐/.test(line)) {
    metrics.milestones.syncCompletedAt ??= receivedAt;
  }
  if (/Session ended/.test(line)) {
    metrics.milestones.sessionEndedAt ??= receivedAt;
  }

  let match = line.match(
    /\[NdjsonFrame\] topic=(\S+) msgs=(\d+) wire_bytes=(\d+)/,
  );
  if (match) {
    metrics.frames.push({
      topic: match[1],
      messages: Number(match[2]),
      wireBytes: Number(match[3]),
    });
    return;
  }
  match = line.match(
    /\[NdjsonStream\] topics=(\d+) chunks=(\d+) wire_bytes=(\d+) first_byte_ms=([0-9.]+) last_byte_ms=([0-9.]+)/,
  );
  if (match) {
    metrics.streams.push({
      topics: Number(match[1]),
      chunks: Number(match[2]),
      wireBytes: Number(match[3]),
      firstByteMs: Number(match[4]),
      lastByteMs: Number(match[5]),
    });
    return;
  }
  match = line.match(
    /\[ProfileDetail\] topic=(\S+) msgs=(\d+) \| prepare=(\S+) submit_queue=(\S+) \| total_proc=(\S+)/,
  );
  if (match) {
    metrics.profiles.push({
      topic: match[1],
      messages: Number(match[2]),
      prepareMs: parseRustDurationMs(match[3]),
      submitQueueMs: parseRustDurationMs(match[4]),
      totalProcessMs: parseRustDurationMs(match[5]),
    });
    return;
  }
  match = line.match(
    /operation=(\S+) outcome=(\S+) wait_ms=([0-9.]+) begin_ms=([0-9.]+) hold_ms=([0-9.]+) finish_ms=([0-9.]+)/,
  );
  if (match) {
    metrics.dbWrites.push({
      operation: match[1],
      outcome: match[2],
      waitMs: Number(match[3]),
      beginMs: Number(match[4]),
      holdMs: Number(match[5]),
      finishMs: Number(match[6]),
    });
    return;
  }
  match = line.match(
    /\[DbWriteQueue\] (?:slow )?Flush(?: completed)? session_id=(\d+) latency_ms=([0-9.]+)/,
  );
  if (match) {
    metrics.flushes.push({
      sessionId: Number(match[1]),
      latencyMs: Number(match[2]),
      receivedAt,
    });
  }
}

async function adbExec(adb, serial, args, timeout = 4000) {
  const result = await execFileAsync(adb, ["-s", serial, ...args], {
    encoding: "utf8",
    maxBuffer: 2 * 1024 * 1024,
    timeout,
  });
  return result.stdout || "";
}

function appendJsonLine(stream, value) {
  stream.write(`${JSON.stringify(value)}\n`);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.serial) {
    process.env.ANDROID_SERIAL = args.serial;
  }
  const device = getDeviceInfo();
  const serial = device.serial;
  const adb = findAdb();
  const packagePath = runAdb(["shell", "pm", "path", DEBUG_PACKAGE], {
    allowFailure: true,
  }).trim();
  if (!packagePath.startsWith("package:")) {
    throw new Error(`${DEBUG_PACKAGE} 未安装；采样器不会安装或启动应用`);
  }
  const pidText = runAdb(["shell", "pidof", DEBUG_PACKAGE], {
    allowFailure: true,
  }).trim();
  const pid = Number(pidText.split(/\s+/)[0]);
  if (!Number.isSafeInteger(pid) || pid <= 0) {
    throw new Error(`${DEBUG_PACKAGE} 当前未运行；请先完成 android:debug:dev`);
  }

  const outDir = ensureDir(
    path.resolve(
      ROOT,
      args.outDir ||
        path.join("tests", "perf", "reports", `full-sync-${timestamp()}`),
    ),
  );
  const samplePath = path.join(outDir, "samples.ndjson");
  const logPath = path.join(outDir, "sync-metrics.log");
  const summaryPath = path.join(outDir, "summary.json");
  const samples = fs.createWriteStream(samplePath, { flags: "wx" });
  const logs = fs.createWriteStream(logPath, { flags: "wx" });
  const startedAt = new Date();
  const startedMonotonic = process.hrtime.bigint();
  const metrics = {
    frames: [],
    streams: [],
    profiles: [],
    dbWrites: [],
    flushes: [],
    milestones: {
      phase3StartedAt: null,
      phase3SkippedAt: null,
      phase3CompletedAt: null,
      finalAckAcceptedAt: null,
      syncCompletedAt: null,
      sessionEndedAt: null,
    },
  };
  const mobilePss = [];
  const mobileRss = [];
  const mobileHwm = [];
  const mobileSwap = [];
  const nodeRss = [];
  const cdsRss = [];
  const errors = [];
  let sampleCount = 0;
  let sampleInFlight = false;
  let stopping = false;
  let logBytes = 0;
  let logRemainder = "";
  let interval = null;
  let deadline = null;
  let nextPssAtMs = 0;

  const logcat = spawn(
    adb,
    ["-s", serial, "logcat", "--pid", String(pid), "-T", "1", "-v", "epoch"],
    {
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  logcat.stdout.setEncoding("utf8");
  logcat.stdout.on("data", (chunk) => {
    logRemainder += chunk;
    const lines = logRemainder.split(/\r?\n/);
    logRemainder = lines.pop() || "";
    for (const line of lines) {
      if (!lineIsAllowed(line)) {
        continue;
      }
      const bytes = Buffer.byteLength(line) + 1;
      if (logBytes + bytes > MAX_CAPTURE_BYTES) {
        if (!errors.includes("filtered log reached 8 MiB limit")) {
          errors.push("filtered log reached 8 MiB limit");
        }
        continue;
      }
      logBytes += bytes;
      logs.write(`${line}\n`);
      updateLogMetrics(line, metrics, new Date().toISOString());
    }
  });
  logcat.stderr.setEncoding("utf8");
  logcat.stderr.on("data", (chunk) => {
    const message = chunk.trim();
    if (message) {
      errors.push(`logcat: ${message}`);
    }
  });

  const sample = async () => {
    if (stopping || sampleInFlight) {
      return;
    }
    sampleInFlight = true;
    const capturedAt = new Date();
    const elapsedMs = Number(process.hrtime.bigint() - startedMonotonic) / 1e6;
    const pssDue = elapsedMs >= nextPssAtMs;
    if (pssDue) {
      do {
        nextPssAtMs += args.pssIntervalMs;
      } while (nextPssAtMs <= elapsedMs);
    }
    try {
      const [statusText, checkinText] = await Promise.all([
        adbExec(adb, serial, [
          "shell",
          "run-as",
          DEBUG_PACKAGE,
          "cat",
          `/proc/${pid}/status`,
        ]),
        pssDue
          ? adbExec(
              adb,
              serial,
              [
                "shell",
                "dumpsys",
                "meminfo",
                "--local",
                "--checkin",
                DEBUG_PACKAGE,
              ],
              10000,
            )
          : Promise.resolve(null),
      ]);
      const mobile = {
        pid,
        ...parseProcStatus(statusText),
        pssKb: checkinText === null ? null : parseCheckinPss(checkinText, pid),
      };
      const node = readHostProcess(args.nodePid);
      const cds = readHostProcess(args.cdsPid);
      if (Number.isFinite(mobile.rssKb)) mobileRss.push(mobile.rssKb);
      if (Number.isFinite(mobile.pssKb)) mobilePss.push(mobile.pssKb);
      if (Number.isFinite(mobile.hwmKb)) mobileHwm.push(mobile.hwmKb);
      if (Number.isFinite(mobile.swapKb)) mobileSwap.push(mobile.swapKb);
      if (Number.isFinite(node?.rssKb)) nodeRss.push(node.rssKb);
      if (Number.isFinite(cds?.rssKb)) cdsRss.push(cds.rssKb);
      appendJsonLine(samples, {
        capturedAt: capturedAt.toISOString(),
        elapsedMs,
        mobile,
        node,
        cds,
      });
      sampleCount += 1;
    } catch (error) {
      errors.push(`sample: ${error.message}`);
    } finally {
      sampleInFlight = false;
    }
  };

  const buildSummary = () => ({
    schema: "vcp.sync-pipeline-report.v1",
    startedAt: startedAt.toISOString(),
    endedAt: new Date().toISOString(),
    durationMs: Number(process.hrtime.bigint() - startedMonotonic) / 1e6,
    intervalMs: args.intervalMs,
    pssIntervalMs: args.pssIntervalMs,
    package: DEBUG_PACKAGE,
    pid,
    device,
    samples: {
      count: sampleCount,
      mobileRssKb: numericSummary(mobileRss),
      mobilePssKb: numericSummary(mobilePss),
      mobileHwmKb: numericSummary(mobileHwm),
      mobileSwapKb: numericSummary(mobileSwap),
      nodeRssKb: numericSummary(nodeRss),
      cdsRssKb: numericSummary(cdsRss),
    },
    ndjson: {
      frameCount: metrics.frames.length,
      totalFrameBytes: metrics.frames.reduce(
        (sum, item) => sum + item.wireBytes,
        0,
      ),
      frameBytes: numericSummary(metrics.frames.map((item) => item.wireBytes)),
      messagesPerFrame: numericSummary(
        metrics.frames.map((item) => item.messages),
      ),
      streams: metrics.streams,
    },
    topicProcessing: {
      topicCount: metrics.profiles.length,
      prepareMs: numericSummary(metrics.profiles.map((item) => item.prepareMs)),
      submitQueueMs: numericSummary(
        metrics.profiles.map((item) => item.submitQueueMs),
      ),
      totalProcessMs: numericSummary(
        metrics.profiles.map((item) => item.totalProcessMs),
      ),
    },
    dbWrites: {
      count: metrics.dbWrites.length,
      failures: metrics.dbWrites.filter((item) => item.outcome !== "committed")
        .length,
      waitMs: numericSummary(metrics.dbWrites.map((item) => item.waitMs)),
      beginMs: numericSummary(metrics.dbWrites.map((item) => item.beginMs)),
      holdMs: numericSummary(metrics.dbWrites.map((item) => item.holdMs)),
      finishMs: numericSummary(metrics.dbWrites.map((item) => item.finishMs)),
    },
    flushes: {
      count: metrics.flushes.length,
      latencyMs: numericSummary(metrics.flushes.map((item) => item.latencyMs)),
      last: metrics.flushes.at(-1) || null,
    },
    milestones: metrics.milestones,
    localProcesses: {
      nodePid: args.nodePid,
      cdsPid: args.cdsPid,
      note:
        args.nodePid || args.cdsPid
          ? "RSS is sampled only for the supplied processes on this host."
          : "Node/CDS RSS not sampled because no local PIDs were supplied.",
    },
    capture: {
      filteredLogBytes: logBytes,
      errors: [...new Set(errors)].slice(0, 50),
      files: {
        samples: path.basename(samplePath),
        syncMetrics: path.basename(logPath),
      },
    },
  });

  const stop = async (reason, exitCode = 0) => {
    if (stopping) {
      return;
    }
    stopping = true;
    if (interval) clearInterval(interval);
    if (deadline) clearTimeout(deadline);
    logcat.kill("SIGTERM");
    const waitStarted = Date.now();
    while (sampleInFlight && Date.now() - waitStarted < 12000) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    samples.end();
    logs.end();
    const summary = buildSummary();
    summary.capture.stopReason = reason;
    fs.writeFileSync(summaryPath, JSON.stringify(summary, null, 2), "utf8");
    console.log(
      `[sync-pipeline] stopped reason=${reason} samples=${sampleCount}`,
    );
    console.log(`[sync-pipeline] report=${outDir}`);
    process.exitCode = exitCode;
  };

  process.on("SIGINT", () => void stop("SIGINT"));
  process.on("SIGTERM", () => void stop("SIGTERM"));
  process.on("uncaughtException", (error) => {
    errors.push(`uncaughtException: ${error.stack || error.message}`);
    void stop("uncaughtException", 1);
  });
  process.on("unhandledRejection", (error) => {
    errors.push(`unhandledRejection: ${error?.stack || error}`);
    void stop("unhandledRejection", 1);
  });

  console.log(
    `[sync-pipeline] package=${DEBUG_PACKAGE} pid=${pid} serial=${serial}`,
  );
  console.log(
    `[sync-pipeline] rss_interval_ms=${args.intervalMs} pss_interval_ms=${args.pssIntervalMs} max_seconds=${args.durationSeconds}`,
  );
  console.log(`[sync-pipeline] report=${outDir}`);
  console.log(
    "[sync-pipeline] READY - start the full sync now; press Ctrl+C after Final ACK",
  );

  await sample();
  interval = setInterval(() => void sample(), args.intervalMs);
  deadline = setTimeout(
    () => void stop("duration-limit"),
    args.durationSeconds * 1000,
  );
}

main().catch((error) => {
  console.error(`[sync-pipeline] ${error.stack || error.message}`);
  process.exitCode = 1;
});
