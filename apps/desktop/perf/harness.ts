import fs from "node:fs/promises";
import path from "node:path";
import type { CDPSession, Page } from "@playwright/test";
import { mediaResponse } from "./media";
import { summarizeSamples, type SampleSummary } from "./stats";
import { raceWithTimeout } from "./timeout-race";

export const F02_DIR = path.resolve(process.cwd(), "test-results/f0-2");

export type AbortedReq = { host: string; resourceType: string };
export type BlockedReq = { origin: string; host: string; resourceType: string };

function requestMeta(request: { url(): string; resourceType(): string }): BlockedReq {
  try {
    const parsed = new URL(request.url());
    return { origin: parsed.origin, host: parsed.host || "unknown", resourceType: request.resourceType() };
  } catch {
    return { origin: "invalid", host: "invalid", resourceType: request.resourceType() };
  }
}

export async function installNetworkGuard(
  page: Page,
  origin: string,
): Promise<{ blocked: BlockedReq[]; aborted: AbortedReq[] }> {
  const allowedOrigin = new URL(origin).origin;
  const blocked: BlockedReq[] = [];
  const aborted: AbortedReq[] = [];
  await page.route("**/*", async (route) => {
    const request = route.request();
    let parsed: URL;
    try {
      parsed = new URL(request.url());
    } catch {
      blocked.push(requestMeta(request));
      await route.abort();
      return;
    }
    if (parsed.origin !== allowedOrigin) {
      blocked.push({ origin: parsed.origin, host: parsed.host || "unknown", resourceType: request.resourceType() });
      await route.abort();
      return;
    }
    const media = mediaResponse(parsed.pathname);
    if (media) {
      await route.fulfill({
        status: 200,
        contentType: media.contentType,
        body: typeof media.body === "string" ? media.body : Buffer.from(media.body),
        headers: { "cache-control": "no-store" },
      });
      return;
    }
    await route.continue();
  });
  page.on("requestfailed", (request) => {
    let parsed: URL;
    try {
      parsed = new URL(request.url());
    } catch {
      return;
    }
    if (parsed.origin === allowedOrigin) {
      aborted.push({ host: parsed.host || "unknown", resourceType: request.resourceType() });
    }
  });
  return { blocked, aborted };
}

export function aggregateAborted(rows: readonly AbortedReq[]): Array<{ host: string; resourceType: string; count: number }> {
  const map = new Map<string, number>();
  for (const row of rows) {
    const key = `${row.host}\t${row.resourceType}`;
    map.set(key, (map.get(key) ?? 0) + 1);
  }
  return [...map.entries()].map(([key, count]) => {
    const [host, resourceType] = key.split("\t");
    return { host: host ?? "unknown", resourceType: resourceType ?? "other", count };
  });
}

export type Timed = { toNextFrameMs: number; toVisibleMs: number | null; scrollDelta?: number };

export type UntilSpec =
  | { kind: "reader"; lastBlock: number; minImages: number }
  | { kind: "chat"; minBubbles: number };

export async function measureUntil(page: Page, clickSelector: string, spec: UntilSpec): Promise<Timed> {
  return page.evaluate(async ({ clickSelector: sel, spec: wait }) => {
    const el = document.querySelector(sel);
    if (!(el instanceof HTMLElement)) throw new Error(`missing ${sel}`);
    const ready = () => {
      if (wait.kind === "reader") {
        const block = document.querySelector(`.drawer.reader [data-block-id="${wait.lastBlock}"]`);
        const images = document.querySelectorAll(".drawer.reader img").length;
        return !!block && images >= wait.minImages;
      }
      return document.querySelectorAll(".chat-pane .bubble").length >= wait.minBubbles;
    };
    const t0 = performance.now();
    el.click();
    await new Promise<void>((resolve, reject) => {
      const deadline = t0 + 30_000;
      const tick = () => {
        if (ready()) {
          resolve();
          return;
        }
        if (performance.now() > deadline) reject(new Error("timeout waiting for condition"));
        else requestAnimationFrame(tick);
      };
      tick();
    });
    const toVisibleMs = performance.now() - t0;
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    });
    return { toNextFrameMs: performance.now() - t0, toVisibleMs };
  }, { clickSelector, spec });
}

export async function measureClick(page: Page, selector: string, waitVisible: string | null = null): Promise<Timed> {
  return page.evaluate(async ({ selector: sel, waitVisible: visible }) => {
    const el = document.querySelector(sel);
    if (!(el instanceof HTMLElement)) throw new Error(`missing ${sel}`);
    const t0 = performance.now();
    el.click();
    let toVisibleMs: number | null = null;
    if (visible) {
      await new Promise<void>((resolve, reject) => {
        const deadline = t0 + 20_000;
        const tick = () => {
          const node = document.querySelector(visible);
          if (node) {
            toVisibleMs = performance.now() - t0;
            resolve();
            return;
          }
          if (performance.now() > deadline) reject(new Error(`timeout waiting ${visible}`));
          else requestAnimationFrame(tick);
        };
        tick();
      });
    }
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    });
    return { toNextFrameMs: performance.now() - t0, toVisibleMs };
  }, { selector, waitVisible });
}

export async function measureScroll(page: Page, selector: string, delta: number): Promise<Timed> {
  return page.evaluate(async ({ selector: sel, delta: dy }) => {
    const el = document.querySelector(sel);
    if (!(el instanceof HTMLElement)) throw new Error(`missing ${sel}`);
    const maxScroll = Math.max(0, el.scrollHeight - el.clientHeight);
    const before = el.scrollTop;
    const step = Math.abs(dy);
    const roomDown = maxScroll - before;
    const roomUp = before;
    let next: number;
    if (roomDown >= 1 && (roomDown >= roomUp || roomUp < 1)) {
      next = Math.min(maxScroll, before + step);
    } else if (roomUp >= 1) {
      next = Math.max(0, before - step);
    } else {
      throw new Error(`scroll no-op: scrollTop=${before} max=${maxScroll}`);
    }
    const t0 = performance.now();
    el.scrollTop = next;
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    });
    const after = el.scrollTop;
    const actual = after - before;
    if (actual === 0) {
      throw new Error(`scrollTop did not change: before=${before} next=${next} after=${after} max=${maxScroll}`);
    }
    return { toNextFrameMs: performance.now() - t0, toVisibleMs: performance.now() - t0, scrollDelta: actual };
  }, { selector, delta });
}

export async function measurePagination(page: Page, selector: string): Promise<Timed> {
  return page.evaluate(async ({ selector: sel }) => {
    const el = document.querySelector(sel);
    if (!(el instanceof HTMLElement)) throw new Error(`missing ${sel}`);
    const before = window.__rrssPerf?.listedCount() ?? 0;
    const t0 = performance.now();
    el.scrollTop = el.scrollHeight;
    await new Promise<void>((resolve, reject) => {
      const deadline = t0 + 20_000;
      const tick = () => {
        const listed = window.__rrssPerf?.listedCount() ?? 0;
        if (listed > before) {
          resolve();
          return;
        }
        if (el.scrollHeight > el.clientHeight && el.scrollTop < el.scrollHeight - el.clientHeight - 4) {
          el.scrollTop = el.scrollHeight;
        }
        if (performance.now() > deadline) reject(new Error("timeout waiting for pagination"));
        else requestAnimationFrame(tick);
      };
      tick();
    });
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    });
    return { toNextFrameMs: performance.now() - t0, toVisibleMs: performance.now() - t0 };
  }, { selector });
}

export async function loadAllConfiguredArticles(page: Page, expected: number): Promise<Timed[]> {
  const hops: Timed[] = [];
  const maxHops = Math.ceil(expected / 5) + 20;
  for (let guard = 0; guard < maxHops; guard++) {
    const state = await page.evaluate(() => ({
      listed: window.__rrssPerf?.listedCount() ?? 0,
      hasMore: window.__rrssPerf?.hasMore() ?? false,
    }));
    if (state.listed === expected) return hops;
    if (state.listed > expected) {
      throw new Error(`listed ${state.listed} exceeded configured ${expected}`);
    }
    if (!state.hasMore) {
      throw new Error(`pagination stopped at listed=${state.listed} expected=${expected}`);
    }
    hops.push(await measurePagination(page, ".focus .scroll"));
  }
  throw new Error(`pagination exceeded ${maxHops} hops`);
}

export async function measureInput(page: Page, selector: string, text: string): Promise<Timed> {
  return page.evaluate(async ({ selector: sel, text: value }) => {
    const el = document.querySelector(sel);
    if (!(el instanceof HTMLTextAreaElement) && !(el instanceof HTMLInputElement)) {
      throw new Error(`missing input ${sel}`);
    }
    const t0 = performance.now();
    el.focus();
    const setter = Object.getOwnPropertyDescriptor(
      el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(el, value);
    if (!setter) el.value = value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    if (el.value !== value) throw new Error(`composer no-op: expected ${value} got ${el.value}`);
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    });
    return { toNextFrameMs: performance.now() - t0, toVisibleMs: performance.now() - t0 };
  }, { selector, text });
}

export type StreamTimingResult = {
  samples: number[];
  sendToNextFrameMs: number;
  emitted: number;
  observed: number;
  matched: number;
  missed: number;
};

export async function measureStreamSnapshots(
  page: Page,
  expected: number,
  sendSelector: string,
): Promise<StreamTimingResult> {
  return page.evaluate(async ({ expected: count, sendSelector: sel }) => {
    const pane = document.querySelector(".chat-pane") ?? document.querySelector("main");
    const send = document.querySelector(sel);
    if (!pane) throw new Error("missing chat pane");
    if (!(send instanceof HTMLElement)) throw new Error(`missing ${sel}`);
    const obsLog: Array<{ renderedAt: number; textLength: number }> = [];
    const note = () => {
      const bubble = [...document.querySelectorAll(".chat-pane .bubble.assistant")].at(-1);
      obsLog.push({ renderedAt: performance.now(), textLength: bubble?.textContent?.length ?? 0 });
    };
    const observer = new MutationObserver(note);
    observer.observe(pane, { subtree: true, childList: true, characterData: true });
    const tSend = performance.now();
    send.click();
    const sendToNextFrameMs = await new Promise<number>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve(performance.now() - tSend)));
    });
    const settled = () => {
      const perf = window.__rrssPerf;
      if (!perf?.streamDone()) return false;
      const emits = perf.streamLog();
      if (emits.length < count) return false;
      if (document.querySelector(".chat-status")) return false;
      const last = emits[emits.length - 1];
      if (!last) return false;
      const bubble = [...document.querySelectorAll(".chat-pane .bubble.assistant")].at(-1);
      return (bubble?.textContent?.length ?? 0) === last.textLength;
    };
    const deadline = tSend + 60_000;
    try {
      while (!settled()) {
        if (performance.now() > deadline) {
          const emits = window.__rrssPerf?.streamLog() ?? [];
          const last = emits[emits.length - 1];
          const bubble = [...document.querySelectorAll(".chat-pane .bubble.assistant")].at(-1);
          throw new Error(
            `timeout waiting for stream settle emits=${emits.length} done=${window.__rrssPerf?.streamDone() ?? false} status=${Boolean(document.querySelector(".chat-status"))} lastLen=${last?.textLength ?? 0} bubbleLen=${bubble?.textContent?.length ?? 0}`,
          );
        }
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      }
    } finally {
      observer.disconnect();
    }
    note();
    const emits = window.__rrssPerf?.streamLog() ?? [];
    const used = new Set<number>();
    const samples: number[] = [];
    let matched = 0;
    for (const emit of emits) {
      let found = -1;
      for (let i = 0; i < obsLog.length; i++) {
        if (used.has(i)) continue;
        const obs = obsLog[i]!;
        if (obs.renderedAt >= emit.emittedAt && obs.textLength === emit.textLength) {
          found = i;
          break;
        }
      }
      if (found >= 0) {
        used.add(found);
        samples.push(obsLog[found]!.renderedAt - emit.emittedAt);
        matched += 1;
      }
    }
    return {
      samples,
      sendToNextFrameMs,
      emitted: emits.length,
      observed: obsLog.length,
      matched,
      missed: emits.length - matched,
    };
  }, { expected, sendSelector });
}

export async function verifyReaderImages(
  page: Page,
  expected: number,
  timeoutMs: number,
): Promise<{ present: number; decoded: number }> {
  return page.evaluate(async ({ expected: want, timeoutMs: budget, raceSource }) => {
    const race = eval(`(${raceSource})`) as (work: Promise<unknown>, ms: number, message?: string) => Promise<unknown>;
    const deadline = performance.now() + budget;
    const imgs = [...document.querySelectorAll(".drawer.reader img")];
    let decoded = 0;
    for (const node of imgs) {
      if (!(node instanceof HTMLImageElement)) continue;
      const remaining = Math.max(0, deadline - performance.now());
      try {
        await race(node.decode(), remaining, "img decode timeout");
        if (node.naturalWidth > 0) decoded += 1;
      } catch {
        // 失败不计成功，不把超时当成解码完成。
      }
    }
    if (imgs.length !== want) {
      throw new Error(`reader img count ${imgs.length} != ${want}`);
    }
    return { present: imgs.length, decoded };
  }, { expected, timeoutMs, raceSource: raceWithTimeout.toString() });
}

export type CdpSnap = {
  scriptDurationMs: number;
  jsHeapUsedSize: number;
  nodes: number;
};

export async function openCdp(page: Page): Promise<CDPSession | null> {
  try {
    const client = await page.context().newCDPSession(page);
    await client.send("Performance.enable");
    return client;
  } catch {
    return null;
  }
}

export async function readCdp(client: CDPSession | null): Promise<CdpSnap | null> {
  if (!client) return null;
  try {
    const result = (await client.send("Performance.getMetrics")) as { metrics?: Array<{ name: string; value: number }> };
    const map = new Map((result.metrics ?? []).map((row) => [row.name, row.value]));
    const script = map.get("ScriptDuration");
    const heap = map.get("JSHeapUsedSize");
    const nodes = map.get("Nodes");
    if (script === undefined || heap === undefined || nodes === undefined) return null;
    return { scriptDurationMs: script * 1000, jsHeapUsedSize: heap, nodes };
  } catch {
    return null;
  }
}

export async function sampleLoopWithCdp(
  client: CDPSession | null,
  warmup: number,
  samples: number,
  run: () => Promise<Timed>,
): Promise<{ interaction: SampleSummary; visible: SampleSummary; script: SampleSummary | { unmeasured: true } }> {
  for (let i = 0; i < warmup; i++) await run();
  const interaction: number[] = [];
  const visible: number[] = [];
  const script: number[] = [];
  let measured = true;
  for (let i = 0; i < samples; i++) {
    const before = await readCdp(client);
    const timed = await run();
    const after = await readCdp(client);
    interaction.push(timed.toNextFrameMs);
    if (timed.toVisibleMs !== null) visible.push(timed.toVisibleMs);
    if (!before || !after) measured = false;
    else script.push(Math.max(0, after.scriptDurationMs - before.scriptDurationMs));
  }
  return {
    interaction: summarizeSamples(interaction),
    visible: summarizeSamples(visible),
    script: measured && script.length > 0 ? summarizeSamples(script) : { unmeasured: true },
  };
}

export async function writeJson(file: string, data: unknown): Promise<void> {
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, `${JSON.stringify(data, null, 2)}\n`, "utf8");
}

export async function screenshot(page: Page, name: string): Promise<void> {
  await fs.mkdir(path.join(F02_DIR, "screenshots"), { recursive: true });
  await page.screenshot({
    path: path.join(F02_DIR, "screenshots", name),
    animations: "disabled",
  });
}
