import path from "node:path";
import { mkdir } from "node:fs/promises";
import { expect, test, type Page } from "@playwright/test";
import { generateScene } from "./generate";
import { MESSAGE_PAGE_DEFAULT, installStrictPerfHost, sceneToHostPayload, summarizeIpc } from "./host";
import { METRIC_MEANINGS, PERF_REPORT_NOTES, machineInfo, type PerfReport, type ScenarioReport } from "./report";
import { IMAGE_DECODE_TIMEOUT_MS, LONG_ARTICLE_TITLE, selectedScenes } from "./scenes";
import { checkTarget, collectOverruns, summarizeSamples } from "./stats";
import {
  F02_DIR,
  aggregateAborted,
  installNetworkGuard,
  loadAllConfiguredArticles,
  measureClick,
  measureInput,
  measureScroll,
  measureStreamSnapshots,
  measureUntil,
  openCdp,
  readCdp,
  sampleLoopWithCdp,
  screenshot,
  verifyReaderImages,
  writeJson,
} from "./harness";

const ORIGIN = "http://127.0.0.1:1431";
const NAV = {
  focus: 'nav[aria-label="主视图"] button:nth-of-type(1)',
  digest: 'nav[aria-label="主视图"] button:nth-of-type(2)',
  chat: 'nav[aria-label="主视图"] button:nth-of-type(3)',
} as const;

function scenarioReport(
  name: string,
  sampled: Awaited<ReturnType<typeof sampleLoopWithCdp>>,
  targetMs: number,
  note?: string,
): ScenarioReport {
  return {
    name,
    phase: "hot",
    interaction: sampled.interaction,
    visible: sampled.visible,
    scriptDurationDelta: sampled.script,
    targetMs,
    note,
  };
}

async function waitFirstArticle(page: Page): Promise<number> {
  await page.getByRole("article").first().waitFor({ state: "visible", timeout: 30_000 });
  const firstArticleMs = await page.evaluate(() => window.__rrssPerfCold?.firstArticleMs ?? null);
  if (firstArticleMs == null) throw new Error("cold observer missed first article");
  return firstArticleMs;
}

async function closeReader(page: Page): Promise<void> {
  if (await page.locator(".drawer.reader").count()) {
    await page.keyboard.press("Escape");
    await expect(page.locator(".drawer.reader")).toHaveCount(0);
  }
}

async function readerCounts(page: Page): Promise<{ title: string; blocks: number; images: number }> {
  return page.evaluate(() => ({
    title: document.querySelector(".drawer.reader .reader-title")?.textContent ?? "",
    blocks: document.querySelectorAll(".drawer.reader [data-block-id]").length,
    images: document.querySelectorAll(".drawer.reader img").length,
  }));
}

async function openLongArticle(page: Page): Promise<void> {
  await page.locator(NAV.focus).click();
  await page.locator(".focus .scroll").evaluate((el) => {
    el.scrollTop = 0;
  });
  await expect(page.locator(`article.article[aria-label="${LONG_ARTICLE_TITLE}"]`)).toBeVisible();
}

for (const sceneName of selectedScenes(process.env.PERF_SCENE)) {
  test.describe(`F0.2 ${sceneName}`, () => {
    test(`production baseline ${sceneName}`, async ({ page, browser }) => {
      const scene = generateScene(sceneName);
      const payload = sceneToHostPayload(scene);
      const { warmup, samples, streamSnapshotCount, targets } = scene.spec;
      const network = await installNetworkGuard(page, ORIGIN);
      const pageErrors: string[] = [];
      page.on("pageerror", (err) => pageErrors.push(String(err)));
      await installStrictPerfHost(page, payload);
      await mkdir(path.join(F02_DIR, "traces"), { recursive: true });

      const context = page.context();
      let tracing = false;
      await context.tracing.start({ screenshots: false, snapshots: false, sources: false });
      tracing = true;
      const client = await openCdp(page);
      const overruns: ReturnType<typeof checkTarget>[] = [];
      const scenarios: Record<string, ScenarioReport> = {};
      let streamMeta = { emitted: 0, observed: 0, matched: 0, missed: 0 };
      let loaded = {
        articlesListed: 0,
        articlesConfigured: scene.spec.articleCount,
        chatBubbles: 0,
        historyMessagesConfigured: scene.spec.historyMessageCount,
        readerBlocks: 0,
        readerImages: 0,
        readerImagesDecoded: 0,
        readerBlocksConfigured: scene.spec.bodyBlockCount,
        readerImagesConfigured: scene.spec.imageCount,
      };

      try {
        await page.goto("/", { waitUntil: "domcontentloaded" });
        const coldStartMs = await waitFirstArticle(page);

        const pageHops = await loadAllConfiguredArticles(page, scene.spec.articleCount);
        loaded.articlesListed = await page.evaluate(() => window.__rrssPerf?.listedCount() ?? 0);
        expect(loaded.articlesListed).toBe(scene.spec.articleCount);
        const pageSamples = pageHops.map((hop) => hop.toNextFrameMs);
        const listPageSummary = summarizeSamples(pageSamples);
        scenarios.listPage = {
          name: "listPage",
          phase: "hot",
          interaction: listPageSummary,
          visible: summarizeSamples(pageHops.map((hop) => hop.toVisibleMs ?? hop.toNextFrameMs)),
          targetMs: targets.interactionP95Ms,
          note: `全量分页 hops=${pageHops.length} listed=${loaded.articlesListed}`,
        };
        overruns.push(checkTarget("listPage.interactionP95", listPageSummary.p95, targets.interactionP95Ms));

        await page.locator(".focus .scroll").evaluate((el) => {
          el.scrollTop = 0;
        });
        const scrollDeltas: number[] = [];
        let scrollCalls = 0;
        const listScroll = await sampleLoopWithCdp(client, warmup, samples, async () => {
          const timed = await measureScroll(page, ".focus .scroll", 480);
          if (timed.scrollDelta == null || timed.scrollDelta === 0) {
            throw new Error("scroll sample did not report a non-zero delta");
          }
          scrollCalls += 1;
          if (scrollCalls > warmup) scrollDeltas.push(timed.scrollDelta);
          return timed;
        });
        expect(scrollDeltas).toHaveLength(samples);
        expect(scrollDeltas.every((delta) => delta !== 0)).toBe(true);
        scenarios.listScroll = {
          ...scenarioReport("listScroll", listScroll, targets.interactionP95Ms),
          note: `往返滚动 actualDeltas=${scrollDeltas.join(",")}`,
        };
        overruns.push(checkTarget("listScroll.interactionP95", listScroll.interaction.p95, targets.interactionP95Ms));

        await openLongArticle(page);
        await page.locator(`article.article[aria-label="${LONG_ARTICLE_TITLE}"]`).click();
        await expect(page.locator(".drawer.reader .reader-title")).toHaveText(LONG_ARTICLE_TITLE);
        const asserted = await readerCounts(page);
        expect(asserted.title).toBe(LONG_ARTICLE_TITLE);
        expect(asserted.blocks).toBe(scene.spec.bodyBlockCount);
        expect(asserted.images).toBe(scene.spec.imageCount);
        const decoded = await verifyReaderImages(page, scene.spec.imageCount, IMAGE_DECODE_TIMEOUT_MS);
        expect(decoded.present).toBe(scene.spec.imageCount);
        expect(decoded.decoded).toBe(scene.spec.imageCount);
        loaded.readerBlocks = asserted.blocks;
        loaded.readerImages = asserted.images;
        loaded.readerImagesDecoded = decoded.decoded;
        await closeReader(page);

        const readerOpen = await sampleLoopWithCdp(client, warmup, samples, async () => {
          await closeReader(page);
          await openLongArticle(page);
          const timed = await measureUntil(page, `article.article[aria-label="${LONG_ARTICLE_TITLE}"]`, {
            kind: "reader",
            lastBlock: scene.spec.bodyBlockCount - 1,
            minImages: scene.spec.imageCount,
          });
          await closeReader(page);
          return timed;
        });
        scenarios.readerOpen = scenarioReport("readerOpen", readerOpen, targets.interactionP95Ms);
        overruns.push(checkTarget("readerOpen.interactionP95", readerOpen.interaction.p95, targets.interactionP95Ms));

        const chatLoad = await sampleLoopWithCdp(client, warmup, samples, async () => {
          await page.locator(NAV.focus).click();
          await expect(page.locator(".chat-pane")).toHaveCount(0);
          return measureUntil(page, ".thread-list button.thread-item:not(.new-thread)", {
            kind: "chat",
            minBubbles: Math.min(scene.spec.historyMessageCount, MESSAGE_PAGE_DEFAULT),
          });
        });
        scenarios.chatLoad = scenarioReport("chatLoad", chatLoad, targets.interactionP95Ms);
        overruns.push(checkTarget("chatLoad.interactionP95", chatLoad.interaction.p95, targets.interactionP95Ms));
        loaded.chatBubbles = await page.evaluate(() => document.querySelectorAll(".chat-pane .bubble").length);
        // 首屏只渲染最新一页历史；更早的消息按需向前加载，DOM 载荷有上界。
        expect(loaded.chatBubbles).toBe(Math.min(scene.spec.historyMessageCount, MESSAGE_PAGE_DEFAULT));

        await page.locator(".thread-list button.thread-item:not(.new-thread)").first().click();
        await expect(page.locator(".chat-pane .bubble").first()).toBeVisible();
        let inputSeq = 0;
        const chatInput = await sampleLoopWithCdp(client, warmup, samples, () =>
          measureInput(page, 'textarea[aria-label="对话消息"]', `合成输入-${inputSeq++}`),
        );
        scenarios.chatInput = scenarioReport("chatInput", chatInput, targets.interactionP95Ms);
        overruns.push(checkTarget("chatInput.interactionP95", chatInput.interaction.p95, targets.interactionP95Ms));

        await page.locator('textarea[aria-label="对话消息"]').fill(`合成流式-${sceneName}`);
        await expect(page.getByRole("button", { name: "发送" })).toBeEnabled();
        const streamBefore = await readCdp(client);
        const stream = await measureStreamSnapshots(page, streamSnapshotCount, ".composer button.primary");
        const streamAfter = await readCdp(client);
        expect(stream.emitted).toBe(streamSnapshotCount);
        streamMeta = {
          emitted: stream.emitted,
          observed: stream.observed,
          matched: stream.matched,
          missed: stream.missed,
        };
        const streamSummary = summarizeSamples(stream.samples);
        scenarios.chatStream = {
          name: "chatStream",
          phase: "hot",
          interaction: streamSummary,
          visible: summarizeSamples([stream.sendToNextFrameMs]),
          scriptDurationDelta:
            streamBefore && streamAfter
              ? summarizeSamples([Math.max(0, streamAfter.scriptDurationMs - streamBefore.scriptDurationMs)])
              : { unmeasured: true },
          targetMs: targets.interactionP95Ms,
          note: `emit→render matched=${stream.matched} missed=${stream.missed} observed=${stream.observed}`,
        };
        overruns.push(checkTarget("chatStream.emitToRenderP95", streamSummary.p95, targets.interactionP95Ms));

        const focusToDigest = await sampleLoopWithCdp(client, warmup, samples, async () => {
          await page.locator(NAV.focus).click();
          await expect(page.locator(".focus")).toBeVisible();
          return measureClick(page, NAV.digest, ".digest");
        });
        scenarios.viewSwitchFocusToDigest = scenarioReport(
          "viewSwitchFocusToDigest",
          focusToDigest,
          targets.interactionP95Ms,
        );
        overruns.push(
          checkTarget("viewSwitchFocusToDigest.interactionP95", focusToDigest.interaction.p95, targets.interactionP95Ms),
        );

        const digestToChat = await sampleLoopWithCdp(client, warmup, samples, async () => {
          await page.locator(NAV.digest).click();
          await expect(page.locator(".digest")).toBeVisible();
          return measureClick(page, NAV.chat, ".chat-pane");
        });
        scenarios.viewSwitchDigestToChat = scenarioReport(
          "viewSwitchDigestToChat",
          digestToChat,
          targets.interactionP95Ms,
        );
        overruns.push(
          checkTarget("viewSwitchDigestToChat.interactionP95", digestToChat.interaction.p95, targets.interactionP95Ms),
        );

        const chatToFocus = await sampleLoopWithCdp(client, warmup, samples, async () => {
          await page.locator(NAV.chat).click();
          await expect(page.locator(".chat-pane")).toBeVisible();
          return measureClick(page, NAV.focus, ".focus");
        });
        scenarios.viewSwitchChatToFocus = scenarioReport(
          "viewSwitchChatToFocus",
          chatToFocus,
          targets.interactionP95Ms,
        );
        overruns.push(
          checkTarget("viewSwitchChatToFocus.interactionP95", chatToFocus.interaction.p95, targets.interactionP95Ms),
        );

        overruns.push(checkTarget("jsWorkP95", null, targets.jsWorkP95Ms, true));

        const audit = await page.evaluate(
          () => window.__rrssPerf?.audit() ?? { unexpected: [], ipc: [], streamEmits: 0, listedCount: 0 },
        );
        expect(audit.unexpected, `unknown ipc: ${audit.unexpected.join(",")}`).toEqual([]);
        expect(network.blocked, `off-origin requests: ${JSON.stringify(network.blocked)}`).toEqual([]);
        expect(network.aborted, `aborted origin requests: ${JSON.stringify(network.aborted)}`).toEqual([]);
        expect(pageErrors, pageErrors.join("\n")).toEqual([]);
        const cdpEnd = await readCdp(client);
        const report: PerfReport = {
          scene: sceneName,
          spec: scene.spec,
          targets,
          machine: machineInfo(browser.version()),
          metricMeanings: METRIC_MEANINGS,
          notes: PERF_REPORT_NOTES,
          coldStart: {
            firstArticleMs: coldStartMs,
            source: "mutation-observer",
            phase: "cold",
            timeOrigin: "document-performance-now",
          },
          loaded,
          stream: streamMeta,
          scenarios,
          overruns: collectOverruns(overruns),
          ipc: {
            mock: true,
            rust: false,
            methods: summarizeIpc(audit.ipc),
          },
          cdp: {
            scriptDurationTotalMs: cdpEnd?.scriptDurationMs ?? null,
            jsHeapUsedSize: cdpEnd?.jsHeapUsedSize ?? null,
            nodes: cdpEnd?.nodes ?? null,
            jsP95Ms: null,
            jsP95Status: "unmeasured",
          },
          abortedRequests: aggregateAborted(network.aborted),
          pageerrorCount: pageErrors.length,
          mediaProtocol: "loopback-http-fixture",
        };
        await writeJson(path.join(F02_DIR, `${sceneName}.json`), report);
        await writeJson(path.join(F02_DIR, `${sceneName}-overruns.json`), report.overruns);

        await page.locator(NAV.focus).click();
        await screenshot(page, `${sceneName}-focus.png`);
        await openLongArticle(page);
        await page.locator(`article.article[aria-label="${LONG_ARTICLE_TITLE}"]`).click();
        await expect(page.locator(".drawer.reader .reader-title")).toBeVisible();
        await screenshot(page, `${sceneName}-reader.png`);
        await closeReader(page);
        await page.locator(".thread-list button.thread-item:not(.new-thread)").first().click();
        await expect(page.locator(".chat-pane .bubble").first()).toBeVisible();
        await screenshot(page, `${sceneName}-chat.png`);

        expect(report.scene).toBe(sceneName);
        expect(report.ipc.mock).toBe(true);
        expect(report.cdp.jsP95Status).toBe("unmeasured");
        expect(report.abortedRequests).toEqual([]);
        expect(report.pageerrorCount).toBe(0);
        expect(pageErrors, pageErrors.join("\n")).toEqual([]);
        expect(network.aborted).toEqual([]);
        expect(network.blocked).toEqual([]);
      } finally {
        try {
          await page.evaluate(() => window.__rrssPerf?.dispose());
        } finally {
          try {
            if (tracing) {
              await context.tracing.stop({ path: path.join(F02_DIR, "traces", `${sceneName}.zip`) });
            }
          } finally {
            if (client) await client.detach();
          }
        }
      }
    });
  });
}
