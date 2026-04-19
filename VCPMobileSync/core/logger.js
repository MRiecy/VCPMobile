const fs = require("fs");
const path = require("path");

class SyncLogger {
  constructor() {
    this.sessionId = null;
    this.phases = new Map();
    this.logDir = path.join(__dirname, "..", "..", "logs", "sync");

    // Ensure log directory exists
    if (!fs.existsSync(this.logDir)) {
      fs.mkdirSync(this.logDir, { recursive: true });
    }

    // Create log file with timestamp
    this.logFilePath = path.join(this.logDir, `sync_${Date.now()}.log`);
    this.logStream = fs.createWriteStream(this.logFilePath, { flags: "a" });
  }

  startSession() {
    this.sessionId = `sync_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
    this.log(`=== Session ${this.sessionId} started ===`);
    this.writeToFile(`Session ${this.sessionId} started`);
    return this.sessionId;
  }

  startPhase(phase, expected = 0) {
    const phaseData = {
      startedAt: Date.now(),
      expected,
      processed: 0,
      success: 0,
      errors: 0,
      details: [],
    };

    this.phases.set(phase, phaseData);
    this.log(`=== Phase ${phase} START: expected=${expected} ===`);
    this.writeToFile(`Phase ${phase} START: expected=${expected}`);
  }

  logOperation(phase, type, id, result, detail = null) {
    const phaseData = this.phases.get(phase);
    if (!phaseData) return;

    phaseData.processed++;
    if (result === "success") phaseData.success++;
    else if (result === "error") phaseData.errors++;

    const message = detail
      ? `${type}:${id} - ${result} (${detail})`
      : `${type}:${id} - ${result}`;

    this.log(`[${phase}] ${message}`);
    this.writeToFile(`[${phase}] ${message}`);

    phaseData.details.push({
      type,
      id,
      result,
      detail,
      ts: Date.now(),
    });
  }

  completePhase(phase) {
    const phaseData = this.phases.get(phase);
    if (!phaseData) return null;

    const duration = Date.now() - phaseData.startedAt;
    const summary = {
      phase,
      expected: phaseData.expected,
      processed: phaseData.processed,
      success: phaseData.success,
      errors: phaseData.errors,
      duration,
    };

    this.log(
      `=== Phase ${phase} COMPLETE: ` +
        `expected=${summary.expected}, processed=${summary.processed}, ` +
        `success=${summary.success}, errors=${summary.errors}, ` +
        `duration=${summary.duration}ms ===`,
    );

    this.writeToFile(`Phase ${phase} COMPLETE: ${JSON.stringify(summary)}`);
    return summary;
  }

  log(message) {
    const timestamp = new Date().toISOString();
    console.log(`[VCPMobileSync] [${timestamp}] ${message}`);
  }

  writeToFile(message) {
    const timestamp = new Date().toISOString();
    this.logStream.write(`[${timestamp}] ${message}\n`);
  }

  endSession() {
    this.log(`=== Session ${this.sessionId} ended ===`);

    const summaries = [];
    for (const [phase, data] of this.phases) {
      const summary = this.completePhase(phase);
      if (summary) summaries.push(summary);
    }

    // Flush log stream and close after a delay
    setTimeout(() => {
      this.logStream.end();
    }, 1000);

    return summaries;
  }
}

module.exports = { SyncLogger };
