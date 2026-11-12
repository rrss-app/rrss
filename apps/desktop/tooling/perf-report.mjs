import path from "node:path";
import { isDirectRun, parseArgs, readJson } from "./cli.mjs";

function usage() {
  return "usage: perf-report.mjs <report.json> [--baseline old.json] [--strict]";
}

function isObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function isFiniteNumber(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function isNonNegInt(value) {
  return Number.isInteger(value) && value >= 0;
}

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (isObject(value)) {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalize(value[key])]),
    );
  }
  return value;
}

function sameCanonical(a, b) {
  return JSON.stringify(canonicalize(a)) === JSON.stringify(canonicalize(b));
}

function pushIssue(issues, message) {
  issues.push(message);
}

const LEGAL_ALGORITHMS = new Set(["nearest-rank"]);

function numbersClose(a, b) {
  if (a === b) return true;
  if (!isFiniteNumber(a) || !isFiniteNumber(b)) return false;
  return Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a), Math.abs(b));
}

export function nearestRank(samples, p) {
  if (samples.length === 0) return null;
  const sorted = [...samples].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * samples.length) - 1));
  return sorted[index] ?? null;
}

function expectedFromRaw(raw) {
  const n = raw.length;
  if (n === 0) return { min: null, max: null, mean: null, p50: null, p95: null };
  const sum = raw.reduce((acc, value) => acc + value, 0);
  return {
    min: Math.min(...raw),
    max: Math.max(...raw),
    mean: sum / n,
    p50: nearestRank(raw, 0.5),
    p95: nearestRank(raw, 0.95),
  };
}

function checkSampleSummary(issues, label, summary) {
  if (!isObject(summary)) {
    pushIssue(issues, `${label} must be a sample summary object`);
    return;
  }
  if (!isNonNegInt(summary.n)) {
    pushIssue(issues, `${label}.n must be a non-negative integer`);
    return;
  }
  if (!Array.isArray(summary.raw)) {
    pushIssue(issues, `${label}.raw must be an array`);
    return;
  }
  if (summary.raw.length !== summary.n) {
    pushIssue(issues, `${label} sample count mismatch: n=${summary.n} raw=${summary.raw.length}`);
  }
  for (const [index, value] of summary.raw.entries()) {
    if (!isFiniteNumber(value)) {
      pushIssue(issues, `${label}.raw[${index}] is not finite`);
    } else if (value < 0) {
      pushIssue(issues, `${label}.raw[${index}] is negative`);
    }
  }
  if (summary.n === 0) {
    for (const field of ["min", "max", "mean", "p50", "p95"]) {
      if (summary[field] !== null && summary[field] !== undefined) {
        pushIssue(issues, `${label}.${field} must be null when n=0`);
      }
    }
    return;
  }
  if (typeof summary.algorithm !== "string" || !LEGAL_ALGORITHMS.has(summary.algorithm)) {
    pushIssue(issues, `${label}.algorithm must be nearest-rank`);
  }
  const expected = expectedFromRaw(summary.raw);
  for (const field of ["min", "max", "mean", "p50", "p95"]) {
    const value = summary[field];
    if (value === null || value === undefined) {
      pushIssue(issues, `${label}.${field} must not be null when n>0`);
      continue;
    }
    if (!isFiniteNumber(value)) pushIssue(issues, `${label}.${field} is not finite`);
    else if (value < 0) pushIssue(issues, `${label}.${field} is negative`);
    else if (!numbersClose(value, expected[field])) {
      pushIssue(issues, `${label}.${field} does not match nearest-rank of raw`);
    }
  }
}

function checkUnmeasuredOrSummary(issues, label, value) {
  if (isObject(value) && value.unmeasured === true) return "unmeasured";
  checkSampleSummary(issues, label, value);
  return "summary";
}

export function validatePerfReport(report) {
  const issues = [];
  if (!isObject(report)) return ["report must be an object"];
  if (typeof report.scene !== "string" || !report.scene.trim()) {
    pushIssue(issues, "missing scene");
  }
  if (!isObject(report.spec)) {
    pushIssue(issues, "missing spec");
  } else {
    if (typeof report.spec.name === "string" && report.scene && report.spec.name !== report.scene) {
      pushIssue(issues, `scene "${report.scene}" does not match spec.name "${report.spec.name}"`);
    }
    if (!isNonNegInt(report.spec.samples)) pushIssue(issues, "spec.samples must be a non-negative integer");
    const targets = report.spec.targets ?? report.targets;
    if (!isObject(targets)) {
      pushIssue(issues, "missing spec.targets");
    } else {
      for (const field of ["interactionP95Ms", "jsWorkP95Ms", "reviewThreshold"]) {
        if (!isFiniteNumber(targets[field])) pushIssue(issues, `spec.targets.${field} is not finite`);
        else if (targets[field] < 0) pushIssue(issues, `spec.targets.${field} is negative`);
      }
    }
  }
  if (!isObject(report.machine)) {
    pushIssue(issues, "missing machine");
  } else {
    for (const field of ["os", "cpu", "node", "build"]) {
      if (typeof report.machine[field] !== "string" || !report.machine[field]) {
        pushIssue(issues, `machine.${field} missing`);
      }
    }
    if (!isNonNegInt(report.machine.cpuCores) || report.machine.cpuCores < 1) {
      pushIssue(issues, "machine.cpuCores missing");
    }
    if (!isObject(report.machine.browser) || typeof report.machine.browser.name !== "string" || typeof report.machine.browser.version !== "string") {
      pushIssue(issues, "machine.browser missing");
    }
  }
  if (!isObject(report.metricMeanings)) pushIssue(issues, "missing metricMeanings");
  if (!isObject(report.scenarios) || Object.keys(report.scenarios).length === 0) {
    pushIssue(issues, "missing scenarios");
  } else {
    for (const [name, scenario] of Object.entries(report.scenarios)) {
      if (!isObject(scenario)) {
        pushIssue(issues, `scenario ${name} is not an object`);
        continue;
      }
      if (scenario.phase !== "hot" && scenario.phase !== "cold") {
        pushIssue(issues, `scenario ${name} has invalid phase`);
      }
      checkSampleSummary(issues, `${name}.interaction`, scenario.interaction);
      if (scenario.visible !== undefined) checkSampleSummary(issues, `${name}.visible`, scenario.visible);
      if (scenario.scriptDurationDelta !== undefined) {
        checkUnmeasuredOrSummary(issues, `${name}.scriptDurationDelta`, scenario.scriptDurationDelta);
      }
    }
  }
  if (report.cdp !== undefined) {
    if (!isObject(report.cdp)) pushIssue(issues, "cdp must be an object");
    else {
      const js = report.cdp.jsP95Ms;
      if (js !== null && js !== undefined) {
        if (!isFiniteNumber(js)) pushIssue(issues, "cdp.jsP95Ms is not finite");
        else if (js < 0) pushIssue(issues, "cdp.jsP95Ms is negative");
      }
      const total = report.cdp.scriptDurationTotalMs;
      if (total !== null && total !== undefined) {
        if (!isFiniteNumber(total)) pushIssue(issues, "cdp.scriptDurationTotalMs is not finite");
        else if (total < 0) pushIssue(issues, "cdp.scriptDurationTotalMs is negative");
      }
    }
  }
  return issues;
}

export function reportTargets(report) {
  return report.spec?.targets ?? report.targets;
}

export function summarizePerfReport(report) {
  const targets = reportTargets(report);
  const scenarios = [];
  const overStart = [];
  const unmeasured = [];

  for (const [name, scenario] of Object.entries(report.scenarios)) {
    const interactionP95 = scenario.interaction?.p95 ?? null;
    const visibleP95 = scenario.visible?.p95 ?? null;
    const n = scenario.interaction?.n ?? 0;
    let script = "n/a";
    if (isObject(scenario.scriptDurationDelta) && scenario.scriptDurationDelta.unmeasured === true) {
      script = "unmeasured";
      unmeasured.push(`${name}.scriptDurationDelta`);
    } else if (scenario.scriptDurationDelta?.p95 == null && scenario.scriptDurationDelta) {
      script = "unmeasured";
      unmeasured.push(`${name}.scriptDurationDelta`);
    } else if (isFiniteNumber(scenario.scriptDurationDelta?.p95)) {
      script = scenario.scriptDurationDelta.p95;
    }
    const overInteraction = isFiniteNumber(interactionP95) && isFiniteNumber(targets.interactionP95Ms) && interactionP95 > targets.interactionP95Ms;
    if (overInteraction) overStart.push(`${name}.interactionP95`);
    scenarios.push({
      name,
      phase: scenario.phase,
      n,
      interactionP95,
      visibleP95,
      scriptDurationDeltaP95: script,
      overStart: overInteraction,
    });
  }

  const jsStatus = report.cdp?.jsP95Status;
  const jsP95 = report.cdp?.jsP95Ms ?? null;
  let jsWork;
  const jsUnmeasured = jsStatus === "unmeasured" || jsStatus === "scriptDurationDelta" || jsP95 === null || jsP95 === undefined;
  if (jsUnmeasured) {
    jsWork = { value: null, status: "unmeasured" };
    unmeasured.push("jsWorkP95");
  } else {
    const over = isFiniteNumber(jsP95) && isFiniteNumber(targets.jsWorkP95Ms) && jsP95 > targets.jsWorkP95Ms;
    jsWork = { value: jsP95, status: jsStatus ?? "measured", overStart: over };
    if (over) overStart.push("jsWorkP95");
  }

  return {
    scene: report.scene,
    targets: {
      interactionP95Ms: targets.interactionP95Ms,
      jsWorkP95Ms: targets.jsWorkP95Ms,
      reviewThreshold: targets.reviewThreshold,
    },
    scenarios,
    jsWork,
    overStart,
    unmeasured,
  };
}

export function formatSummary(summary) {
  const lines = [
    `scene: ${summary.scene}`,
    `targets: interactionP95=${summary.targets.interactionP95Ms}ms jsWorkP95=${summary.targets.jsWorkP95Ms}ms reviewThreshold=${summary.targets.reviewThreshold}`,
    "scenarios:",
  ];
  for (const scenario of summary.scenarios) {
    const flag = scenario.overStart ? " 需要复核(超起始目标)" : "";
    lines.push(
      `  ${scenario.name}  ${scenario.phase}  n=${scenario.n}  interactionP95=${scenario.interactionP95}  visibleP95=${scenario.visibleP95}  scriptDeltaP95=${scenario.scriptDurationDeltaP95}${flag}`,
    );
  }
  if (summary.jsWork.status === "unmeasured") {
    lines.push("jsWorkP95: 未测 (not using cdp.scriptDurationTotalMs or scenario scriptDurationDelta as the 8ms frame metric)");
  } else {
    const flag = summary.jsWork.overStart ? " 需要复核(超起始目标)" : "";
    lines.push(`jsWorkP95: ${summary.jsWork.value} (${summary.jsWork.status})${flag}`);
  }
  lines.push(`over starting targets: ${summary.overStart.length ? summary.overStart.join(", ") : "none"}`);
  lines.push(`unmeasured: ${summary.unmeasured.length ? summary.unmeasured.join(", ") : "none"}`);
  return `${lines.join("\n")}\n`;
}

function metricPresence(value) {
  if (value === undefined) return "missing";
  if (isObject(value) && value.unmeasured === true) return "unmeasured";
  if (isObject(value) && Array.isArray(value.raw)) return "measured";
  return "invalid";
}

function pushPresenceIssue(issues, label, current, baseline) {
  if (current !== baseline) {
    issues.push(`incomparable ${label}: ${current} vs ${baseline}`);
  }
}

export function comparabilityIssues(current, baseline) {
  const issues = [];
  const fields = [
    ["machine.os", current.machine?.os, baseline.machine?.os],
    ["machine.cpu", current.machine?.cpu, baseline.machine?.cpu],
    ["machine.cpuCores", current.machine?.cpuCores, baseline.machine?.cpuCores],
    ["machine.node", current.machine?.node, baseline.machine?.node],
    ["machine.build", current.machine?.build, baseline.machine?.build],
    ["machine.browser.name", current.machine?.browser?.name, baseline.machine?.browser?.name],
    ["machine.browser.version", current.machine?.browser?.version, baseline.machine?.browser?.version],
  ];
  for (const [label, a, b] of fields) {
    if (a !== b) issues.push(`incomparable ${label}: ${a} vs ${b}`);
  }
  if (!sameCanonical(current.spec, baseline.spec)) issues.push("incomparable scene spec");
  if (!sameCanonical(current.metricMeanings, baseline.metricMeanings)) issues.push("incomparable metric meanings");

  const currentNames = new Set(Object.keys(current.scenarios ?? {}));
  const baselineNames = new Set(Object.keys(baseline.scenarios ?? {}));
  for (const name of currentNames) {
    if (!baselineNames.has(name)) issues.push(`incomparable missing baseline scenario ${name}`);
  }
  for (const name of baselineNames) {
    if (!currentNames.has(name)) issues.push(`incomparable missing current scenario ${name}`);
  }
  for (const name of currentNames) {
    if (!baselineNames.has(name)) continue;
    const a = current.scenarios[name];
    const b = baseline.scenarios[name];
    if (a.phase !== b.phase) issues.push(`incomparable ${name} phase: ${a.phase} vs ${b.phase}`);
    if (a.interaction?.n !== b.interaction?.n) {
      issues.push(`incomparable ${name} sample count: ${a.interaction?.n} vs ${b.interaction?.n}`);
    }
    pushPresenceIssue(issues, `${name}.visible`, metricPresence(a.visible), metricPresence(b.visible));
    pushPresenceIssue(issues, `${name}.scriptDurationDelta`, metricPresence(a.scriptDurationDelta), metricPresence(b.scriptDurationDelta));
  }
  return issues;
}

function reviewIfRegressed(reviews, metric, current, baseline, threshold) {
  if (current === null || current === undefined || baseline === null || baseline === undefined) {
    reviews.push({ metric, status: "incomparable-metric" });
    return;
  }
  const delta = baseline === 0 ? (current === 0 ? 0 : Infinity) : (current - baseline) / baseline;
  if (delta > threshold) {
    reviews.push({ metric, current, baseline, delta, needsReview: true });
  }
}

export function compareReports(current, baseline) {
  const reviews = [];
  const threshold = reportTargets(current).reviewThreshold;
  for (const name of Object.keys(current.scenarios)) {
    const a = current.scenarios[name];
    const b = baseline.scenarios[name];
    reviewIfRegressed(reviews, `${name}.interactionP95`, a.interaction?.p95, b.interaction?.p95, threshold);
    if (metricPresence(a.visible) === "measured" && metricPresence(b.visible) === "measured") {
      reviewIfRegressed(reviews, `${name}.visibleP95`, a.visible.p95, b.visible.p95, threshold);
    }
    if (metricPresence(a.scriptDurationDelta) === "measured" && metricPresence(b.scriptDurationDelta) === "measured") {
      reviewIfRegressed(reviews, `${name}.scriptDurationDeltaP95`, a.scriptDurationDelta.p95, b.scriptDurationDelta.p95, threshold);
    }
  }

  const aJs = current.cdp?.jsP95Ms;
  const bJs = baseline.cdp?.jsP95Ms;
  const aUnmeasured = current.cdp?.jsP95Status === "unmeasured" || current.cdp?.jsP95Status === "scriptDurationDelta" || aJs === null || aJs === undefined;
  const bUnmeasured = baseline.cdp?.jsP95Status === "unmeasured" || baseline.cdp?.jsP95Status === "scriptDurationDelta" || bJs === null || bJs === undefined;
  if (aUnmeasured !== bUnmeasured) {
    reviews.push({ metric: "jsWorkP95", status: "incomparable-metric" });
  } else if (!aUnmeasured) {
    reviewIfRegressed(reviews, "jsWorkP95", aJs, bJs, threshold);
  }
  return reviews;
}

export function loadPerfReport(file) {
  const report = readJson(file);
  const issues = validatePerfReport(report);
  if (issues.length) {
    const error = new Error(`invalid report ${file}:\n${issues.map((item) => `  ${item}`).join("\n")}`);
    error.issues = issues;
    throw error;
  }
  return report;
}

export function runPerfReport(argv, io = process) {
  let args;
  try {
    args = parseArgs(argv, {
      flags: ["strict"],
      values: ["baseline"],
    });
  } catch (err) {
    io.stderr.write(`${err instanceof Error ? err.message : err}\n${usage()}\n`);
    return 1;
  }
  if (args.positional.length !== 1) {
    io.stderr.write(`${usage()}\n`);
    return 1;
  }

  const reportPath = path.resolve(args.positional[0]);
  let report;
  try {
    report = loadPerfReport(reportPath);
  } catch (err) {
    io.stderr.write(`${err instanceof Error ? err.message : err}\n`);
    return 1;
  }

  const summary = summarizePerfReport(report);
  io.stdout.write(formatSummary(summary));

  let needsReview = summary.overStart.length > 0;
  if (args.values.baseline) {
    const baselinePath = path.resolve(args.values.baseline);
    let baseline;
    try {
      baseline = loadPerfReport(baselinePath);
    } catch (err) {
      io.stderr.write(`${err instanceof Error ? err.message : err}\n`);
      return 1;
    }
    const incomparable = comparabilityIssues(report, baseline);
    if (incomparable.length) {
      io.stderr.write(`incomparable reports:\n${incomparable.map((item) => `  ${item}`).join("\n")}\n`);
      return 1;
    }
    const reviews = compareReports(report, baseline);
    const metricGaps = reviews.filter((item) => item.status === "incomparable-metric");
    if (metricGaps.length) {
      io.stderr.write(`incomparable metrics:\n${metricGaps.map((item) => `  ${item.metric}`).join("\n")}\n`);
      return 1;
    }
    const flagged = reviews.filter((item) => item.needsReview);
    if (flagged.length) {
      needsReview = true;
      io.stdout.write("baseline comparison:\n");
      for (const item of flagged) {
        io.stdout.write(`  ${item.metric} ${item.baseline} → ${item.current} 需要复核\n`);
      }
    } else {
      io.stdout.write("baseline comparison: no reviewThreshold regressions\n");
    }
  }

  if (needsReview && args.flags.strict) {
    io.stderr.write("strict: review required\n");
    return 1;
  }
  return 0;
}

if (isDirectRun(import.meta.url)) {
  process.exit(runPerfReport(process.argv.slice(2)));
}
