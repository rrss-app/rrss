import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";
import {
  assertForwardMessagePages,
  assertFocusPages,
  buildPerfReport,
  collectForwardMessages,
  finalizeHostReport,
  perfConsoleLine,
  perfProbeScript,
  scaleFromScenes,
  summarizeMessageWalk,
  utf8JsonBytes,
} from "./perf-metrics.mjs";

const ROOT = new URL("..", import.meta.url);

test("utf8 json bytes count chinese and emoji, not utf16 code units", () => {
  assert.equal(JSON.stringify("中").length, 3);
  assert.equal(utf8JsonBytes("中"), 5);
  assert.equal(JSON.stringify("😀").length, 4);
  assert.equal(utf8JsonBytes("😀"), 6);
  const value = { title: "合成", mark: "😀" };
  const json = JSON.stringify(value);
  const utf8 = new TextEncoder().encode(json).length;
  assert.notEqual(json.length, utf8);
  assert.equal(utf8JsonBytes(value), utf8);
});

test("payload bytes are labeled a serialized estimate, not transport", () => {
  const report = sampleReport();
  assert.equal(report.payloadBytes.kind, "serialized-json-utf8-estimate");
  assert.match(report.payloadBytes.note, /not transport/);
  assert.equal(report.performanceClaim, "not-a-pass");
  assert.equal(report.percentile, null);
  assert.match(report.percentileReason, /not p95/);
  assert.match(report.definitions.scrollHeightReadMs, /Not layout/);
  assert.match(report.definitions.paintEntries, /not a paint benchmark/);
  assert.equal(report.jsHeap, null);
  assert.match(report.jsHeapReason, /performance\.memory/);
  assert.equal(report.layoutMs, undefined);
  assert.equal(report.focus.p95, undefined);
});

test("probe script parses as a function body", () => {
  const script = perfProbeScript(expectFixture());
  new Function(`return ${script}`);
});

test("probe script does not measure payload with string length", () => {
  const script = perfProbeScript(expectFixture());
  assert.equal(script.includes("JSON.stringify(value).length"), false);
  assert.match(script, /new TextEncoder\(\)/);
  assert.match(script, /collectForwardMessages/);
  assert.match(script, /assertFocusPages/);
  assert.match(script, /duplicate focus id/);
  assert.match(script, /reader block count/);
  assert.match(script, /focus_stream/);
  assert.match(script, /article_reader/);
  assert.match(script, /Not the UI history loader/);
});

test("focus page assertions still reject a drifted page", () => {
  assert.throws(() => assertFocusPages(
    [{ count: 40, hasMore: true }, { count: 10, hasMore: false }],
    50, 80, 40,
  ), /focus count/);
  assert.throws(() => assertFocusPages(
    [{ count: 20, hasMore: true }, { count: 40, hasMore: false }],
    80, 80, 40,
  ), /focus page size drifted/);
  assert.doesNotThrow(() => assertFocusPages(
    [{ count: 40, hasMore: true }, { count: 40, hasMore: false }],
    80, 80, 40,
  ));
});

test("message walk keeps first page and full total apart", async () => {
  const calls = [];
  const pages = await collectForwardMessages(async (beforeId) => {
    calls.push(beforeId);
    return windowPage(45, 40, beforeId);
  }, 45, 40);
  assert.deepEqual(calls, [null, 6]);
  const summary = summarizeMessageWalk(pages);
  assert.equal(summary.firstPage.count, 40);
  assert.equal(summary.firstPage.hasMore, true);
  assert.deepEqual(pages[0].ids, Array.from({ length: 40 }, (_, i) => i + 6));
  assert.equal(summary.full.count, 45);
  assert.equal(summary.full.pages, 2);
  assert.deepEqual(summary.full.pageCounts, [40, 5]);
  assert.match(summary.note, /Not the UI history loader/);
  const ordered = pages.slice().reverse().flatMap((page) => page.ids);
  assert.deepEqual(ordered, Array.from({ length: 45 }, (_, i) => i + 1));
  const onePage = await collectForwardMessages(async (beforeId) => windowPage(20, 40, beforeId), 20, 40);
  const oneSummary = summarizeMessageWalk(onePage);
  assert.equal(onePage.length, 1);
  assert.equal(oneSummary.firstPage.count, 20);
  assert.equal(oneSummary.firstPage.hasMore, false);
  assert.equal(oneSummary.full.count, 20);
});

test("message walk rejects duplicates, gaps, unstable order, and oversize pages", async () => {
  await assert.rejects(
    collectForwardMessages(async () => ({ ids: [1, 1], hasMore: false, ms: 0, payloadBytes: 0 }), 2, 40),
    /unstable message order/,
  );
  await assert.rejects(
    collectForwardMessages(async () => ({ ids: [1, 2, 4], hasMore: false, ms: 0, payloadBytes: 0 }), 3, 40),
    /missing or unstable message id/,
  );
  assert.throws(
    () => assertForwardMessagePages([
      { ids: [2, 3], hasMore: true },
      { ids: [1, 2], hasMore: false },
    ], 4, 2),
    /duplicate message id/,
  );
  await assert.rejects(
    collectForwardMessages(async () => ({ ids: [2, 1], hasMore: false, ms: 0, payloadBytes: 0 }), 2, 40),
    /unstable message order/,
  );
  await assert.rejects(
    collectForwardMessages(async () => ({ ids: Array.from({ length: 41 }, (_, i) => i + 1), hasMore: false, ms: 0, payloadBytes: 0 }), 41, 40),
    /exceeds bound/,
  );
  await assert.rejects(
    collectForwardMessages(async () => ({ ids: [1], hasMore: true, ms: 0, payloadBytes: 0 }), 1, 40),
    /did not reach the end/,
  );
});

test("single-sample report keeps a real heap and refuses a pass", () => {
  const report = sampleReport({ jsHeap: { usedJsHeap: 3, totalJsHeap: 4 }, jsHeapReason: null });
  assert.deepEqual(report.jsHeap, { usedJsHeap: 3, totalJsHeap: 4 });
  assert.equal(report.jsHeapReason, null);
  const saved = finalizeHostReport(report, { os: "Linux test", cpu: "cpu x1" });
  assert.equal(saved.environment.os, "Linux test");
  assert.equal(saved.environment.cpu, "cpu x1");
  assert.equal(saved.environment.mode, "debug+vite-dev");
  assert.equal(saved.environment.strictMode, "enabled");
  assert.match(saved.environment.engineUserAgent, /TestWebKit/);
  assert.throws(() => finalizeHostReport({ ...sampleReport(), performanceClaim: "pass" }, { os: "Linux", cpu: "cpu" }), /not claim a performance pass/);
});

test("console line has counts only", () => {
  const line = perfConsoleLine(finalizeHostReport(sampleReport(), { os: "Linux", cpu: "cpu x1" }));
  assert.match(line, /not-a-pass/);
  assert.match(line, /single-sample/);
  assert.match(line, /serialized-json-utf8-estimate/);
  assert.equal(line.includes("sk-"), false);
  assert.equal(line.includes("合成段落"), false);
  assert.equal(line.includes("p95"), false);
});

test("scaleFromScenes follows the scene object", () => {
  const scenes = fixtureScenes();
  assert.equal(scaleFromScenes(scenes).minimal.messages, 20);
  assert.equal(scaleFromScenes(scenes).minimal.mockPageSize, 20);
  const drifted = {
    ...scenes,
    daily: { ...scenes.daily, historyMessageCount: 199 },
  };
  assert.equal(scaleFromScenes(drifted).daily.messages, 199);
  const partial = { ...scenes, stress: { ...scenes.stress } };
  delete partial.stress.bodyBlockCount;
  assert.throws(() => scaleFromScenes(partial), /bodyBlockCount/);
});

test("live scenes.ts is the scale the config script prints", () => {
  const source = readFileSync(new URL("./perf-scene-config.mjs", import.meta.url), "utf8");
  assert.match(source, /from "\.\.\/apps\/desktop\/perf\/scenes\.ts"/);
  assert.match(source, /scaleFromScenes\(SCENES/);
  const proc = spawnSync(process.execPath, ["--experimental-strip-types", "scripts/perf-scene-config.mjs"], {
    cwd: new URL(".", ROOT),
    encoding: "utf8",
  });
  assert.equal(proc.status, 0, proc.stderr);
  assert.deepEqual(JSON.parse(proc.stdout), {
    minimal: { articles: 80, messages: 20, blocks: 40, images: 4, mockPageSize: 20, streamSnapshots: 60, streamIntervalMs: 30 },
    daily: { articles: 500, messages: 200, blocks: 400, images: 24, mockPageSize: 40, streamSnapshots: 60, streamIntervalMs: 30 },
    stress: { articles: 3000, messages: 800, blocks: 1200, images: 64, mockPageSize: 40, streamSnapshots: 60, streamIntervalMs: 30 },
  });
});

test("dev entry still wraps StrictMode", () => {
  const main = readFileSync(new URL("./apps/desktop/src/main.tsx", ROOT), "utf8");
  assert.match(main, /<StrictMode>/);
});

function windowPage(total, pageSize, beforeId) {
  const eligible = [];
  for (let id = 1; id <= total; id++) {
    if (beforeId == null || id < beforeId) eligible.push(id);
  }
  return {
    ids: eligible.slice(Math.max(0, eligible.length - pageSize)),
    hasMore: eligible.length > pageSize,
    ms: 1.26,
    payloadBytes: 8,
  };
}

function fixtureScenes() {
  const spec = (articleCount, historyMessageCount, bodyBlockCount, imageCount, pageSize) => ({
    articleCount, historyMessageCount, bodyBlockCount, imageCount, pageSize,
    streamSnapshotCount: 60, streamIntervalMs: 30,
  });
  return {
    minimal: spec(80, 20, 40, 4, 20),
    daily: spec(500, 200, 400, 24, 40),
    stress: spec(3000, 800, 1200, 64, 40),
  };
}

function expectFixture() {
  return {
    scene: "minimal", articles: 80, messages: 20, blocks: 40, images: 4,
    pageSize: 40, messagePage: 40, threadId: 1001, articleId: 1, dbBytes: 1,
  };
}

function sampleReport(overrides = {}) {
  const messages = summarizeMessageWalk([{
    ids: [1], hasMore: false, ms: 1.26, payloadBytes: 8,
  }]);
  return buildPerfReport({
    scene: "minimal",
    dbBytes: 10,
    userAgent: "TestWebKit",
    viteDevClient: true,
    focusPages: [{ count: 40, hasMore: true, ms: 1.24, payloadBytes: 10 }, { count: 40, hasMore: false, ms: 1, payloadBytes: 11 }],
    messages,
    reader: { blocks: 40, images: 4, ms: 2, payloadBytes: 12 },
    domArticles: 18,
    scrollHeight: 100,
    scrollHeightReadMs: 0.04,
    paintEntries: [],
    jsHeap: null,
    jsHeapReason: "performance.memory is not available on this engine",
    ...overrides,
  });
}
