import fs from "node:fs";
import os from "node:os";
import path from "node:path";

function writeFiles(dir, files) {
  for (const [rel, content] of Object.entries(files)) {
    const abs = path.join(dir, rel);
    fs.mkdirSync(path.dirname(abs), { recursive: true });
    fs.writeFileSync(abs, content);
  }
  return dir;
}

function rmTemp(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
}

export function nearestRank(samples, p) {
  if (samples.length === 0) return null;
  const sorted = [...samples].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * samples.length) - 1))] ?? null;
}

export function sampleSummary(raw, algorithm = "nearest-rank") {
  const n = raw.length;
  return {
    n,
    raw: [...raw],
    min: n === 0 ? null : Math.min(...raw),
    max: n === 0 ? null : Math.max(...raw),
    mean: n === 0 ? null : raw.reduce((acc, value) => acc + value, 0) / n,
    p50: nearestRank(raw, 0.5),
    p95: nearestRank(raw, 0.95),
    algorithm,
  };
}

export function createFixtureKit() {
  const dirs = [];

  function remember(dir) {
    dirs.push(dir);
    return dir;
  }

  return {
    makeFixture(files) {
      return writeFiles(remember(fs.mkdtempSync(path.join(os.tmpdir(), "rrss-f03-"))), files);
    },
    writeReport(name, report) {
      const dir = remember(fs.mkdtempSync(path.join(os.tmpdir(), "rrss-f03-perf-")));
      const file = path.join(dir, name);
      fs.writeFileSync(file, `${JSON.stringify(report, null, 2)}\n`);
      return file;
    },
    cleanup() {
      for (const dir of dirs.splice(0)) rmTemp(dir);
    },
  };
}

export function captureIo() {
  let stdout = "";
  let stderr = "";
  return {
    stdout: { write(chunk) { stdout += chunk; return true; } },
    stderr: { write(chunk) { stderr += chunk; return true; } },
    get stdoutText() { return stdout; },
    get stderrText() { return stderr; },
  };
}

export function validPerfReport(overrides = {}) {
  const interaction = sampleSummary([10, 12]);
  const report = {
    scene: "minimal",
    spec: {
      name: "minimal",
      seed: 20092026,
      samples: 2,
      articleCount: 80,
      targets: { interactionP95Ms: 100, jsWorkP95Ms: 8, reviewThreshold: 0.2 },
    },
    targets: { interactionP95Ms: 100, jsWorkP95Ms: 8, reviewThreshold: 0.2 },
    machine: {
      os: "linux 1 x64",
      cpu: "test-cpu",
      cpuCores: 8,
      node: "v24.14.0",
      browser: { name: "chromium", version: "1.0.0" },
      build: "production",
    },
    metricMeanings: {
      interactionToNextFrameMs: "dispatch to next frame",
      jsWork: "per-interaction js p95",
    },
    scenarios: {
      listScroll: {
        name: "listScroll",
        phase: "hot",
        interaction,
      },
    },
    cdp: {
      scriptDurationTotalMs: 9999,
      jsHeapUsedSize: 1,
      nodes: 1,
      jsP95Ms: null,
      jsP95Status: "unmeasured",
    },
  };
  return structuredClone({ ...report, ...overrides, spec: { ...report.spec, ...(overrides.spec ?? {}) } });
}
