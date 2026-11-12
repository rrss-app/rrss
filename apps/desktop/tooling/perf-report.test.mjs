import assert from "node:assert/strict";
import path from "node:path";
import os from "node:os";
import { after, test } from "node:test";
import {
  formatSummary,
  runPerfReport,
  summarizePerfReport,
  validatePerfReport,
} from "./perf-report.mjs";
import { captureIo, createFixtureKit, sampleSummary, validPerfReport } from "./fixture.mjs";

const { writeReport, cleanup } = createFixtureKit();
after(cleanup);

function run(argv) {
  const io = captureIo();
  const code = runPerfReport(argv, io);
  return { code, io };
}

test("argument errors fail without dumping JSON", () => {
  const unknown = run(["--wat"]);
  assert.equal(unknown.code, 1);
  assert.match(unknown.io.stderrText, /unknown argument/);
  assert.doesNotMatch(unknown.io.stderrText, /"metricMeanings"/);

  const missing = run([]);
  assert.equal(missing.code, 1);
  assert.match(missing.io.stderrText, /usage/);

  const absent = run([path.join(os.tmpdir(), "rrss-missing-perf.json")]);
  assert.equal(absent.code, 1);
  assert.match(absent.io.stderrText, /cannot read/);
});

test("rejects non-finite, negative, sample mismatch, missing scenes, and inconsistent stats", () => {
  const nan = validPerfReport();
  nan.scenarios.listScroll.interaction.p95 = Number.NaN;
  assert.ok(validatePerfReport(nan).some((item) => item.includes("p95")));

  const negative = validPerfReport();
  negative.scenarios.listScroll.interaction.raw = [10, -1];
  negative.scenarios.listScroll.interaction.n = 2;
  assert.ok(validatePerfReport(negative).some((item) => item.includes("negative")));

  const mismatch = validPerfReport();
  mismatch.scenarios.listScroll.interaction.n = 8;
  mismatch.scenarios.listScroll.interaction.raw = [1, 2];
  assert.ok(validatePerfReport(mismatch).some((item) => item.includes("sample count mismatch")));

  const missing = validPerfReport();
  missing.scenarios = {};
  assert.ok(validatePerfReport(missing).some((item) => item.includes("missing scenarios")));

  const nullStat = validPerfReport();
  nullStat.scenarios.listScroll.interaction.p95 = null;
  assert.ok(validatePerfReport(nullStat).some((item) => item.includes("must not be null when n>0")));

  const wrongRank = validPerfReport();
  wrongRank.scenarios.listScroll.interaction.p95 = 10;
  assert.ok(validatePerfReport(wrongRank).some((item) => item.includes("nearest-rank")));

  const badAlgo = validPerfReport();
  badAlgo.scenarios.listScroll.interaction.algorithm = "linear-interpolation";
  assert.ok(validatePerfReport(badAlgo).some((item) => item.includes("algorithm")));

  const nameMismatch = validPerfReport();
  nameMismatch.scene = "daily";
  assert.ok(validatePerfReport(nameMismatch).some((item) => item.includes("does not match spec.name")));

  const file = writeReport("bad.json", mismatch);
  const result = run([file]);
  assert.equal(result.code, 1);
  assert.doesNotMatch(result.io.stderrText, /"metricMeanings"/);
});

test("starting-target overrun is reported but not a failure; --strict fails", () => {
  const report = validPerfReport();
  report.scenarios.listScroll.interaction = sampleSummary([150, 160]);
  const file = writeReport("over.json", report);
  const ordinary = run([file]);
  assert.equal(ordinary.code, 0);
  assert.match(ordinary.io.stdoutText, /需要复核/);
  assert.match(ordinary.io.stdoutText, /listScroll\.interactionP95/);
  assert.doesNotMatch(ordinary.io.stdoutText, /"metricMeanings"/);

  const strict = run([file, "--strict"]);
  assert.equal(strict.code, 1);
});

test("does not treat cumulative or action scriptDuration as the 8ms jsP95 metric", () => {
  const report = validPerfReport();
  report.cdp = {
    scriptDurationTotalMs: 9999,
    jsP95Ms: 50,
    jsP95Status: "scriptDurationDelta",
  };
  report.scenarios.listScroll.scriptDurationDelta = sampleSummary([40, 50]);
  const summary = summarizePerfReport(report);
  assert.equal(summary.jsWork.status, "unmeasured");
  assert.ok(summary.unmeasured.includes("jsWorkP95"));
  assert.ok(!summary.overStart.includes("jsWorkP95"));
  const text = formatSummary(summary);
  assert.match(text, /未测/);
  assert.match(text, /not using cdp\.scriptDurationTotalMs/);
  assert.match(text, /scriptDurationDelta/);
});

test("comparable baseline regression needs review; different env is incomparable", () => {
  const current = validPerfReport();
  current.scenarios.listScroll.interaction = sampleSummary([40, 50]);
  const old = validPerfReport();
  old.scenarios.listScroll.interaction = sampleSummary([10, 12]);
  const currentFile = writeReport("current.json", current);
  const oldFile = writeReport("old.json", old);
  const cmp = run([currentFile, "--baseline", oldFile]);
  assert.equal(cmp.code, 0);
  assert.match(cmp.io.stdoutText, /需要复核/);
  const strict = run([currentFile, "--baseline", oldFile, "--strict"]);
  assert.equal(strict.code, 1);

  const otherCpu = validPerfReport();
  otherCpu.machine = { ...otherCpu.machine, cpu: "other-cpu" };
  const otherFile = writeReport("other.json", otherCpu);
  const incomparable = run([currentFile, "--baseline", otherFile]);
  assert.equal(incomparable.code, 1);
  assert.match(incomparable.io.stderrText, /incomparable machine\.cpu/);

  const otherSpec = validPerfReport();
  otherSpec.spec = { ...otherSpec.spec, samples: 99 };
  const specFile = writeReport("spec.json", otherSpec);
  const specCmp = run([currentFile, "--baseline", specFile]);
  assert.equal(specCmp.code, 1);
  assert.match(specCmp.io.stderrText, /incomparable scene spec/);
});

test("compare rejects phase, sample-count, missing visible/script, and measured-to-unmeasured gaps", () => {
  const base = validPerfReport();
  base.scenarios.listScroll.visible = sampleSummary([10, 12]);
  base.scenarios.listScroll.scriptDurationDelta = sampleSummary([4, 5]);
  const baseFile = writeReport("base-full.json", base);

  const phase = validPerfReport();
  phase.scenarios.listScroll.phase = "cold";
  phase.scenarios.listScroll.visible = sampleSummary([10, 12]);
  phase.scenarios.listScroll.scriptDurationDelta = sampleSummary([4, 5]);
  assert.equal(run([writeReport("phase.json", phase), "--baseline", baseFile]).code, 1);

  const samples = validPerfReport();
  samples.scenarios.listScroll.interaction = sampleSummary([10, 12, 14]);
  samples.scenarios.listScroll.visible = sampleSummary([10, 12]);
  samples.scenarios.listScroll.scriptDurationDelta = sampleSummary([4, 5]);
  const sampleCmp = run([writeReport("n.json", samples), "--baseline", baseFile]);
  assert.equal(sampleCmp.code, 1);
  assert.match(sampleCmp.io.stderrText, /sample count/);

  const missingVisible = validPerfReport();
  missingVisible.scenarios.listScroll.scriptDurationDelta = sampleSummary([4, 5]);
  const visCmp = run([writeReport("novis.json", missingVisible), "--baseline", baseFile]);
  assert.equal(visCmp.code, 1);
  assert.match(visCmp.io.stderrText, /visible/);

  const unmeasured = validPerfReport();
  unmeasured.scenarios.listScroll.visible = sampleSummary([10, 12]);
  unmeasured.scenarios.listScroll.scriptDurationDelta = { unmeasured: true };
  const unmeasCmp = run([writeReport("unmeas.json", unmeasured), "--baseline", baseFile]);
  assert.equal(unmeasCmp.code, 1);
  assert.match(unmeasCmp.io.stderrText, /scriptDurationDelta/);
});

test("compares visible and action scriptDelta regressions without treating them as 8ms jsP95", () => {
  const old = validPerfReport();
  old.scenarios.listScroll.visible = sampleSummary([10, 12]);
  old.scenarios.listScroll.scriptDurationDelta = sampleSummary([4, 5]);
  const current = validPerfReport();
  current.scenarios.listScroll.visible = sampleSummary([20, 30]);
  current.scenarios.listScroll.scriptDurationDelta = sampleSummary([10, 20]);
  const result = run([writeReport("vis.json", current), "--baseline", writeReport("vis-old.json", old)]);
  assert.equal(result.code, 0);
  assert.match(result.io.stdoutText, /visibleP95/);
  assert.match(result.io.stdoutText, /scriptDurationDeltaP95/);
  assert.match(result.io.stdoutText, /需要复核/);
  assert.match(result.io.stdoutText, /jsWorkP95: 未测/);
});

test("review threshold is read from spec.targets, not a hardcoded 20%", () => {
  const current = validPerfReport();
  current.spec.targets = { interactionP95Ms: 100, jsWorkP95Ms: 8, reviewThreshold: 0.5 };
  current.targets = current.spec.targets;
  current.scenarios.listScroll.interaction = sampleSummary([12, 14]);
  const old = validPerfReport();
  old.spec.targets = current.spec.targets;
  old.targets = current.spec.targets;
  const currentFile = writeReport("wide.json", current);
  const oldFile = writeReport("wide-old.json", old);
  const result = run([currentFile, "--baseline", oldFile]);
  assert.equal(result.code, 0);
  assert.match(result.io.stdoutText, /no reviewThreshold regressions/);
});

test("extra observed or stream fields do not fail core scenario validation", () => {
  const report = validPerfReport();
  report.observed = { counts: { listed: 80 }, stream: { snapshots: 8 } };
  report.scenarios.listScroll.observed = { streamEmits: 3 };
  assert.deepEqual(validatePerfReport(report), []);
});
