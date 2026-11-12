// Browser-safe perf-report helpers. The probe inlines the function declarations
// into the webview; do not use Node built-ins inside those declarations.
// payload bytes are a serialized JSON estimate, not transport bytes.
import os from "node:os";

export function utf8JsonBytes(value) {
  const json = JSON.stringify(value);
  return new TextEncoder().encode(json).length;
}

export function payloadByteMeta() {
  return {
    kind: "serialized-json-utf8-estimate",
    note: "UTF-8 byte length of JSON.stringify output. Serialized payload estimate, not transport, framing, or on-disk bytes.",
  };
}

export function assertFocusPages(pages, seenCount, total, pageSize) {
  if (seenCount !== total) throw new Error(`focus count ${seenCount} != ${total}`);
  const last = pages[pages.length - 1];
  const remainder = total % pageSize || pageSize;
  if (!last || last.count !== remainder || last.hasMore) throw new Error("last focus page is wrong");
  if (pages.slice(0, -1).some((page) => page.count !== pageSize || !page.hasMore)) {
    throw new Error("focus page size drifted");
  }
}

function roundMs(value) {
  return Math.round(value * 10) / 10;
}

/**
 * Newest page first. Ids within a page are strictly increasing.
 * Stitched oldest-to-newest ids must be exactly 1..total with no gaps.
 * Each page is at most pageSize; only the oldest page may be short.
 */
export function assertForwardMessagePages(pages, total, pageSize) {
  if (!Number.isInteger(total) || total < 1) throw new Error("bad message total");
  if (!Number.isInteger(pageSize) || pageSize < 1) throw new Error("bad message page size");
  if (!pages.length) throw new Error("no message pages");
  const seen = new Set();
  for (let index = 0; index < pages.length; index++) {
    const page = pages[index];
    const ids = page.ids;
    if (ids.length > pageSize) {
      throw new Error(`message page exceeds bound: ${ids.length} > ${pageSize}`);
    }
    for (let i = 1; i < ids.length; i++) {
      if (ids[i] <= ids[i - 1]) throw new Error(`unstable message order on page ${index}`);
    }
    for (const id of ids) {
      if (seen.has(id)) throw new Error(`duplicate message id ${id}`);
      seen.add(id);
    }
    const oldest = index === pages.length - 1;
    if (oldest) {
      if (page.hasMore) throw new Error("oldest message page still has more");
    } else if (!page.hasMore || ids.length !== pageSize) {
      throw new Error("message page ended before the configured total");
    }
  }
  const ordered = pages.slice().reverse().flatMap((page) => page.ids);
  if (ordered.length !== total) throw new Error(`message count ${ordered.length} != ${total}`);
  for (let i = 0; i < ordered.length; i++) {
    if (ordered[i] !== i + 1) throw new Error(`missing or unstable message id at ${i}: ${ordered[i]}`);
  }
  const remainder = total % pageSize || pageSize;
  if (pages[pages.length - 1].ids.length !== remainder) {
    throw new Error("oldest message page size mismatch");
  }
  return { firstPageCount: pages[0].ids.length, fullCount: ordered.length, pages: pages.length };
}

export async function collectForwardMessages(fetchPage, total, pageSize) {
  const pages = [];
  let beforeId = null;
  for (let hop = 0; hop < total + 2; hop++) {
    const page = await fetchPage(beforeId);
    pages.push(page);
    if (!page.hasMore) break;
    if (!page.ids.length) throw new Error("empty message page while more remains");
    beforeId = page.ids[0];
  }
  if (!pages.length || pages[pages.length - 1].hasMore) {
    throw new Error("message pagination did not reach the end");
  }
  assertForwardMessagePages(pages, total, pageSize);
  return pages;
}

export function summarizeMessageWalk(pages) {
  const first = pages[0];
  return {
    note: "IPC forward pagination of the messages command until the configured total. Not the UI history loader.",
    firstPage: {
      count: first.ids.length,
      hasMore: first.hasMore,
      ms: roundMs(first.ms),
      payloadBytes: first.payloadBytes,
    },
    full: {
      count: pages.reduce((sum, page) => sum + page.ids.length, 0),
      pages: pages.length,
      ms: roundMs(pages.reduce((sum, page) => sum + page.ms, 0)),
      payloadBytes: pages.reduce((sum, page) => sum + page.payloadBytes, 0),
      pageCounts: pages.map((page) => page.ids.length),
    },
  };
}

export function buildPerfReport(input) {
  const meta = payloadByteMeta();
  const focusPages = input.focusPages;
  const first = focusPages[0] || { count: 0, hasMore: false, ms: 0, payloadBytes: 0 };
  const sum = (key) => focusPages.reduce((total, page) => total + page[key], 0);
  return {
    scene: input.scene,
    performanceClaim: "not-a-pass",
    samples: 1,
    percentile: null,
    percentileReason: "One IPC wait or one scrollHeight read is a single sample. It is not p95.",
    payloadBytes: meta,
    definitions: {
      payloadBytes: meta.note,
      ipcWaitMs: "performance.now() around one invoke, or the sum of those single waits. Not a sample distribution and not p95.",
      scrollHeightReadMs: "Time to read Element.scrollHeight once. Not layout duration, not paint, and not a frame time.",
      paintEntries: "PerformancePaintTiming entries exposed by the engine, if any. Listing them is not a paint benchmark and not a pass.",
      jsHeap: "performance.memory when the engine exposes it; otherwise null with jsHeapReason.",
      messages: "messages IPC walked with beforeId until the configured total. Not the UI history loader.",
      domArticles: "article.article nodes currently in the document. Not a claim that the UI loaded history or the full list.",
    },
    environment: {
      engineUserAgent: input.userAgent || "",
      viteDevClient: input.viteDevClient === true,
    },
    dbBytes: input.dbBytes,
    focus: {
      firstPage: {
        count: first.count,
        hasMore: first.hasMore,
        ms: roundMs(first.ms),
        payloadBytes: first.payloadBytes,
      },
      pages: focusPages.length,
      articles: focusPages.reduce((total, page) => total + page.count, 0),
      ms: roundMs(sum("ms")),
      payloadBytes: sum("payloadBytes"),
      pageMs: focusPages.map((page) => roundMs(page.ms)),
      pagePayloadBytes: focusPages.map((page) => page.payloadBytes),
    },
    messages: input.messages,
    reader: {
      blocks: input.reader.blocks,
      images: input.reader.images,
      ms: roundMs(input.reader.ms),
      payloadBytes: input.reader.payloadBytes,
    },
    domArticles: input.domArticles,
    scrollHeight: input.scrollHeight,
    scrollHeightReadMs: roundMs(input.scrollHeightReadMs),
    paintEntries: input.paintEntries,
    jsHeap: input.jsHeap,
    jsHeapReason: input.jsHeap ? null : (input.jsHeapReason || "performance.memory is not available on this engine"),
  };
}

export function finalizeHostReport(report, host) {
  if (!report || report.performanceClaim !== "not-a-pass") {
    throw new Error("perf report must not claim a performance pass");
  }
  for (const key of ["p95", "layoutMs", "paintMs", "passed"]) {
    if (report[key] != null) throw new Error(`perf report must not include ${key}`);
  }
  for (const section of ["focus", "messages", "reader"]) {
    if (report[section] && report[section].p95 != null) {
      throw new Error(`perf report must not include ${section}.p95`);
    }
  }
  if (!report.environment?.engineUserAgent) throw new Error("engine user agent missing");
  if (!String(report.definitions?.payloadBytes || "").includes("not transport")) {
    throw new Error("payload byte definition missing");
  }
  if (!String(report.definitions?.scrollHeightReadMs || "").includes("Not layout")) {
    throw new Error("scrollHeight definition missing");
  }
  if (!String(report.messages?.note || "").includes("Not the UI history loader")) {
    throw new Error("message walk labeled as UI history");
  }
  if (report.jsHeap == null && !report.jsHeapReason) throw new Error("missing jsHeap must explain why");
  if (!host?.os || !host?.cpu) throw new Error("host os/cpu missing");
  report.environment = {
    engineUserAgent: report.environment.engineUserAgent,
    os: host.os,
    cpu: host.cpu,
    mode: "debug+vite-dev",
    modeNote: "Debug Tauri binary plus the Vite development server. Not a release build and not the production Vite preview.",
    strictMode: "enabled",
    strictModeNote: "apps/desktop/src/main.tsx renders App inside React StrictMode. This run does not re-detect that from the fiber tree.",
    viteDevClient: report.environment.viteDevClient === true,
  };
  return report;
}

export function perfConsoleLine(report) {
  const { focus, messages, reader } = report;
  return `Perf host ${report.scene}: not-a-pass; single-sample; focus ${focus.articles} articles / ${focus.pages} pages ${focus.ms} ms ${focus.payloadBytes} serialized-json-utf8-estimate bytes; messages first ${messages.firstPage.count} full ${messages.full.count} ${messages.full.ms} ms ${messages.full.payloadBytes} serialized-json-utf8-estimate bytes; reader ${reader.blocks} blocks ${reader.ms} ms ${reader.payloadBytes} serialized-json-utf8-estimate bytes`;
}

const BROWSER_FNS = [
  utf8JsonBytes,
  payloadByteMeta,
  roundMs,
  assertFocusPages,
  assertForwardMessagePages,
  collectForwardMessages,
  summarizeMessageWalk,
  buildPerfReport,
];

export function perfProbeScript(expect) {
  const prelude = BROWSER_FNS.map((fn) => fn.toString()).join("\n");
  return `(async () => {
${prelude}
const expect = ${JSON.stringify(expect)};
const invoke = window.__TAURI_INTERNALS__.invoke;
await invoke('set_list_scope', { scope: 'all' });
await invoke('set_focus_filter', { filter: 'all' });
await invoke('set_setting', { id: 'cluster_group_enabled', value: '0' });
async function timed(run) {
  const started = performance.now();
  const value = await run();
  return { value, ms: performance.now() - started, payloadBytes: utf8JsonBytes(value) };
}
const pages = [];
const seen = new Set();
let excludeIds = [];
for (let hop = 0; hop < expect.articles + 2; hop++) {
  const timedPage = await timed(() => invoke('focus_stream', {
    expandedCluster: null, feedId: null, more: hop > 0,
    excludeIds: hop > 0 ? excludeIds : null, excludeClusterIds: null,
  }));
  const ids = timedPage.value.articles.map((row) => row.id);
  for (const id of ids) {
    if (seen.has(id)) throw new Error('duplicate focus id ' + id);
    seen.add(id);
  }
  pages.push({ ms: timedPage.ms, payloadBytes: timedPage.payloadBytes, count: ids.length, hasMore: timedPage.value.has_more });
  if (!timedPage.value.has_more) break;
  if (!ids.length) throw new Error('empty focus page while more remains');
  excludeIds = excludeIds.concat(ids);
}
if (seen.size !== expect.articles) throw new Error('focus count ' + seen.size + ' != ' + expect.articles);
assertFocusPages(pages, seen.size, expect.articles, expect.pageSize);
const messagePages = await collectForwardMessages(async (beforeId) => {
  const timedPage = await timed(() => invoke('messages', { threadId: expect.threadId, beforeId, limit: null }));
  return {
    ids: timedPage.value.messages.map((row) => row.id),
    hasMore: timedPage.value.has_more === true,
    ms: timedPage.ms,
    payloadBytes: timedPage.payloadBytes,
  };
}, expect.messages, expect.messagePage);
const messages = summarizeMessageWalk(messagePages);
const reader = await timed(() => invoke('article_reader', { id: expect.articleId }));
const blocks = reader.value.document && reader.value.document.blocks;
if (!blocks || blocks.length !== expect.blocks) throw new Error('reader block count');
const images = blocks.filter((block) => block.t === 'img').length;
if (images !== expect.images) throw new Error('reader image count ' + images);
const deadline = performance.now() + 5000;
let domArticles = 0;
while (performance.now() < deadline) {
  domArticles = document.querySelectorAll('article.article').length;
  if (domArticles) break;
  await new Promise((resolve) => setTimeout(resolve, 50));
}
if (!domArticles) throw new Error('seeded focus list did not render');
const scroller = document.querySelector('.focus .scroll');
const scrollStarted = performance.now();
const scrollHeight = scroller ? scroller.scrollHeight : 0;
const scrollHeightReadMs = performance.now() - scrollStarted;
const paintEntries = (performance.getEntriesByType ? performance.getEntriesByType('paint') : [])
  .map((entry) => ({ name: entry.name, startTime: entry.startTime }));
const jsHeap = performance.memory ? {
  usedJsHeap: performance.memory.usedJSHeapSize,
  totalJsHeap: performance.memory.totalJSHeapSize,
} : null;
const report = buildPerfReport({
  scene: expect.scene,
  dbBytes: expect.dbBytes,
  userAgent: navigator.userAgent,
  viteDevClient: !!document.querySelector('script[src*="/@vite/client"]'),
  focusPages: pages,
  messages,
  reader: { blocks: blocks.length, images, ms: reader.ms, payloadBytes: reader.payloadBytes },
  domArticles,
  scrollHeight,
  scrollHeightReadMs,
  paintEntries,
  jsHeap,
  jsHeapReason: jsHeap ? null : 'performance.memory is not available on this engine',
});
return 'rrss-perf-report ' + JSON.stringify(report);
})()`;
}

export function scaleFromScenes(scenes, names = ["minimal", "daily", "stress"]) {
  if (!scenes || typeof scenes !== "object") throw new Error("scenes missing");
  const out = {};
  for (const name of names) {
    const spec = scenes[name];
    if (!spec || typeof spec !== "object") throw new Error(`missing scene ${name}`);
    const articles = requiredCount(spec, "articleCount", name);
    const messages = requiredCount(spec, "historyMessageCount", name);
    const blocks = requiredCount(spec, "bodyBlockCount", name);
    const images = requiredCount(spec, "imageCount", name, 0);
    const mockPageSize = requiredCount(spec, "pageSize", name);
    const streamSnapshots = requiredCount(spec, "streamSnapshotCount", name);
    const streamIntervalMs = requiredCount(spec, "streamIntervalMs", name);
    if (images > blocks) throw new Error(`${name} has more images than blocks`);
    out[name] = { articles, messages, blocks, images, mockPageSize, streamSnapshots, streamIntervalMs };
  }
  const extra = Object.keys(scenes).filter((name) => !names.includes(name));
  if (extra.length) throw new Error(`unknown scenes: ${extra.join(",")}`);
  return out;
}

function requiredCount(spec, key, name, minimum = 1) {
  const value = spec[key];
  if (!Number.isInteger(value) || value < minimum) {
    throw new Error(`${name}.${key} is not an integer >= ${minimum}`);
  }
  return value;
}

export function hostFacts() {
  const cpus = os.cpus();
  const cpu = cpus[0];
  return {
    os: `${os.type()} ${os.release()} ${os.arch()}`,
    cpu: cpu ? `${cpu.model} x${cpus.length}` : "unknown",
  };
}
