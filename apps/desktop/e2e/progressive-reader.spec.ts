import { expect, test } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { progressiveReaderFixture } from "../src/test/fixtures/reader";
import { shot } from "./shots";
import { VIEWPORT_DEBOUNCE_MS } from "../src/features/reader/translationViewport";
import type { TranslationHint, TranslationInitialViewport, TranslationSnapshot } from "../src/bindings";

const fixture = progressiveReaderFixture();
test("viewport hints follow the latest scroll, mode and layout, and stop on close", async ({ page }) => {
  await installMockHost(page, { reader: fixture.reader });
  await page.goto("/");
  await page.evaluate(fixture => {
    const host = (window as any).__TAURI_INTERNALS__, original = host.invoke;
    const state = { starts: [] as TranslationInitialViewport[], hints: [] as TranslationHint[], snapshot: null as TranslationSnapshot | null };
    (window as any).__viewport = state;
    const reply = () => ({ kind: state.snapshot ? "resumed" : "missing", reader: fixture.reader, snapshot: state.snapshot, note: "", reconcile_after_ms: 45000 });
    host.invoke = async (command: string, args: Record<string, any> = {}) => {
      switch (command) {
        case "translate_article": state.starts.push(args.initial); state.snapshot = structuredClone(fixture.first); return { ...reply(), kind: "started" };
        case "translate_snapshot": return reply();
        case "translate_hint": state.hints.push(args.hint); return;
        default: return original(command, args);
      }
    };
  }, fixture);
  const reader = page.locator(".drawer.reader"), body = reader.locator(".drawer-body");
  const state = () => page.evaluate(() => (window as any).__viewport as { starts: TranslationInitialViewport[]; hints: TranslationHint[] });
  const viewport = () => body.evaluate(body => {
    const top = body.getBoundingClientRect().top + body.clientTop, bottom = top + body.clientHeight;
    const visible: number[] = [], nearby: number[] = [];
    for (const node of body.querySelectorAll<HTMLElement>("[data-reader-document] > [data-block-id]")) {
      const rect = node.getBoundingClientRect(), id = Number(node.dataset.blockId);
      if (rect.bottom > top && rect.top < bottom) visible.push(id);
      else if (rect.bottom > top - body.clientHeight && rect.top < bottom + body.clientHeight) nearby.push(id);
    }
    return { visible, nearby };
  });
  const scroll = (index: number) => body.evaluate((body, index) => {
    const node = body.querySelector<HTMLElement>(`[data-reader-document] > [data-block-id="${index}"]`)!;
    body.scrollTop += node.getBoundingClientRect().top - body.getBoundingClientRect().top;
  }, index);
  const synced = () => expect.poll(async () => {
    const latest = await state();
    return JSON.stringify(latest.hints.at(-1)?.viewport ?? latest.starts.at(-1)?.viewport) === JSON.stringify(await viewport());
  }).toBe(true);
  await page.getByRole("article").first().click();
  await expect(reader).toBeVisible();
  await scroll(7);
  const initial = await viewport();
  await page.keyboard.press("t");
  await expect.poll(async () => (await state()).starts.length).toBe(1);
  expect((await state()).starts[0].viewport).toEqual(initial);
  await expect(reader.getByRole("radio", { name: "双语", checked: true })).toBeVisible();
  await synced();
  await page.evaluate(() => { (window as any).__viewport.hints = []; });
  // 三个连续绘制帧内快滑，经过的区域不能变成待发送的提示队列。
  await body.evaluate(async body => {
    for (const index of [14, 2, 9]) {
      const node = body.querySelector<HTMLElement>(`[data-reader-document] > [data-block-id="${index}"]`)!;
      body.scrollTop += node.getBoundingClientRect().top - body.getBoundingClientRect().top;
      await new Promise(requestAnimationFrame);
    }
  });
  await expect.poll(async () => (await state()).hints.length).toBe(1);
  await synced();
  expect((await state()).hints[0].viewport.visible).toContain(9);
  const ticket = (await state()).hints[0].ticket;
  await reader.getByRole("radio", { name: "原文", exact: true }).click();
  await synced();
  await reader.getByRole("radio", { name: "译文", exact: true }).click();
  await synced();
  // 模拟本地图片解码后撑高所在块，无滚动事件时也必须重新观察。
  await reader.locator('[data-reader-document] > [data-block-id="9"]').evaluate(node => { (node as HTMLElement).style.minHeight = "1000px"; });
  await synced();
  await page.setViewportSize({ width: 560, height: 720 });
  await synced();
  const count = (await state()).hints.length;
  await page.waitForTimeout(VIEWPORT_DEBOUNCE_MS * 2);
  expect((await state()).hints).toHaveLength(count);
  await scroll(3);
  await reader.getByRole("button", { name: "返回", exact: true }).click();
  const closed = (await state()).hints.length;
  await page.waitForTimeout(VIEWPORT_DEBOUNCE_MS * 2);
  expect((await state()).hints).toHaveLength(closed);
  await page.getByRole("article").first().click();
  await expect(reader).toBeVisible();
  await expect.poll(async () => (await state()).hints.at(-1)?.ticket).toBeGreaterThan(ticket);
  await synced();
  expect((await state()).starts).toHaveLength(1);
});

test("progressive results, recovery and opt-in diagnostics stay inside the reader", async ({ page }) => {
  await installMockHost(page, { reader: fixture.reader });
  await page.goto("/");
  await page.evaluate(fixture => {
    const host = (window as any).__TAURI_INTERNALS__, mock = (window as any).__mockHost;
    const original = host.invoke;
    const state = { reader: fixture.reader, snapshot: null as any, starts: [] as string[], saves: 0, attaches: 0, detaches: 0, generation: 20 };
    const reply = (kind = state.snapshot ? "resumed" : "missing") => ({ kind, reader: state.reader, snapshot: state.snapshot, note: "", reconcile_after_ms: 45000 });
    const persist = () => {
      const document = structuredClone(fixture.reader.document) as { blocks: unknown[] };
      for (const block of state.snapshot.blocks) document.blocks[block.index] = block.ir;
      state.reader = { ...fixture.reader, translated_title: state.snapshot.title, translated_document: document, translation_ready: true };
    };
    (window as any).__progressive = {
      state,
      push: (name: "first" | "partial" | "complete" | "saveFailed" | "allFailed") => {
        const before = state.snapshot;
        const next = structuredClone(fixture[name]);
        state.snapshot = { ...next, identity: { ...next.identity, generation: state.generation }, seq: (before?.seq ?? 1) + 1 };
        const payload = { ...state.snapshot, kind: "delta", save: "unsaved", error: next.error,
          total: next.units.length, done: next.units.filter(s => s === "done").length,
          failed: next.units.filter(s => typeof s === "object").length, units: next.units.map((s, i) => [i, s]) };
        mock.emit("translate:delta", payload);
        if (next.save === "saved" || next.save === "save_failed") {
          if (next.save === "saved") persist();
          state.snapshot.seq++;
          mock.emit(next.save === "saved" ? "translate:done" : "translate:error", { ...payload, seq: state.snapshot.seq,
            kind: next.save === "saved" ? "done" : "error", save: next.save, title: null, blocks: [], units: [], error: "error" in next ? next.error : null });
        }
      },
    };
    host.invoke = async (command: string, args: Record<string, any> = {}) => {
      switch (command) {
        case "translate_article": {
          state.starts.push(args.mode); state.generation++;
          state.snapshot = structuredClone(args.mode === "retry_failed" ? fixture.retrying : fixture.queued);
          state.snapshot.identity.generation = state.generation;
          return reply("started");
        }
        case "translate_snapshot": return reply();
        case "translate_attach": state.attaches++; return;
        case "translate_detach": state.detaches++; return;
        case "translate_retry_save": state.saves++; state.snapshot.save = "saved"; state.snapshot.error = null; state.snapshot.seq++; persist(); return reply();
        default: return original(command, args);
      }
    };
  }, fixture);
  const open = () => page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  const rail = page.locator(".reader-view-rail");
  const push = (name: "first" | "partial" | "complete" | "saveFailed" | "allFailed") => page.evaluate(name => (window as any).__progressive.push(name), name);
  const anchor = () => reader.locator(".drawer-body").evaluate(body => {
    const top = body.getBoundingClientRect().top;
    const block = [...body.querySelectorAll<HTMLElement>("[data-reader-document] > [data-block-id]")].find(node => node.getBoundingClientRect().bottom > top + 4)!;
    return { id: block.dataset.blockId, top: block.getBoundingClientRect().top, offset: block.getBoundingClientRect().top - top };
  });
  const counts = () => page.evaluate(() => ({ starts: (window as any).__progressive.state.starts, saves: (window as any).__progressive.state.saves }));
  const debug = async (enabled: boolean) => {
    await reader.getByRole("button", { name: "返回", exact: true }).click();
    await page.getByRole("button", { name: "设置", exact: true }).click();
    await settingsTab(page, "advanced").click();
    const setting = page.getByRole("button", { name: "界面调试模式", exact: true });
    await expect(setting).toHaveAttribute("aria-pressed", String(!enabled));
    await setting.click(); await expect(setting).toHaveAttribute("aria-pressed", String(enabled));
    await page.getByRole("dialog", { name: "设置", exact: true }).getByRole("button", { name: "返回阅读", exact: true }).click();
    await open();
  };
  await open();
  await expect(reader).toBeVisible();
  await expect(reader.locator(".reader-translation-status")).toHaveCount(0);
  await reader.getByRole("button", { name: "翻译", exact: true }).click();
  await expect.poll(counts).toEqual({ starts: ["default"], saves: 0 });
  await reader.locator(".drawer-body").evaluate(el => { el.scrollTop = 700; });
  const beforeFirst = await anchor();
  await push("first");
  await expect(rail.getByRole("radio", { name: "双语", checked: true })).toBeVisible();
  expect((await anchor()).id).toBe(beforeFirst.id);
  expect(Math.abs((await anchor()).top - beforeFirst.top)).toBeLessThan(2);
  await reader.locator(".drawer-body").evaluate(el => { el.scrollTop = 0; });
  await expect(reader.getByText(/^第 1 段/)).toBeVisible();
  await expect(reader.locator(".reader-translation-status")).toHaveCount(0);
  await shot(page, "test-results/t1c-reader/default-light.png");

  await rail.getByRole("radio", { name: "原文", exact: true }).click();
  await debug(true);
  await expect(reader.getByRole("progressbar", { name: "翻译进度" })).toBeVisible();
  await expect(rail.getByRole("radio", { name: "原文", checked: true })).toBeVisible();
  await rail.getByRole("radio", { name: "双语", exact: true }).click();
  await shot(page, "test-results/t1c-reader/debug-light.png");
  const statusTop = (await reader.locator(".reader-translation-status").boundingBox())!.y;
  await reader.locator(".drawer-body").evaluate(el => { el.scrollTop = 700; });
  expect((await reader.locator(".reader-translation-status").boundingBox())!.y).toBe(statusTop);
  await debug(false);
  await expect(reader.locator(".reader-translation-status")).toHaveCount(0);
  await rail.getByRole("radio", { name: "双语", exact: true }).click();
  const beforePartial = await anchor();
  const selected = await page.evaluate(() => {
    const text = document.querySelector('[data-block-id="7"] .block-tr p span')!.firstChild!;
    const range = document.createRange(); range.setStart(text, 0); range.setEnd(text, 35);
    const selection = getSelection()!; selection.removeAllRanges(); selection.addRange(range); return selection.toString();
  });
  await push("partial");
  await expect.poll(() => page.evaluate(() => getSelection()?.toString())).toBe(selected);
  await expect(reader.locator('[data-block-id="7"] .block-tr')).toContainText("Part 8");
  await page.evaluate(() => getSelection()?.removeAllRanges());
  await expect(reader.locator('[data-block-id="7"] .block-tr')).toContainText("第 8 段");
  expect((await anchor()).id).toBe(beforePartial.id);
  expect(Math.abs((await anchor()).top - beforePartial.top)).toBeLessThan(2);
  await expect(reader.getByRole("button", { name: "重试未译部分", exact: true }).first()).toBeVisible();
  await expect(reader.getByRole("progressbar")).toHaveCount(0);
  await reader.getByRole("button", { name: "查看未译处", exact: true }).click();
  await expect(reader.locator('[data-block-id="3"]')).toContainText("未译出");
  await page.setViewportSize({ width: 560, height: 920 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  await shot(page, "test-results/t1c-reader/partial-dark-narrow.png");
  await reader.getByRole("button", { name: "重试未译部分", exact: true }).first().click();
  await expect.poll(counts).toEqual({ starts: ["default", "retry_failed"], saves: 0 });
  await push("saveFailed");
  await expect(reader.getByRole("button", { name: "重试保存", exact: true }).first()).toBeVisible();
  await reader.getByRole("button", { name: "重试保存", exact: true }).first().click();
  await expect.poll(counts).toEqual({ starts: ["default", "retry_failed"], saves: 1 });
  await expect(reader.locator(".reader-translation-status")).toHaveCount(0);
  await reader.locator(".reader-tools > summary").click();
  await reader.getByRole("button", { name: "重新翻译全文", exact: true }).click();
  const confirm = page.getByRole("alertdialog", { name: "重新翻译全文？" });
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "取消", exact: true }).click();
  expect((await counts()).starts).toHaveLength(2);
  await reader.locator(".reader-tools > summary").click();
  await reader.getByRole("button", { name: "重新翻译全文", exact: true }).click();
  await confirm.getByRole("button", { name: "确认", exact: true }).click();
  await expect.poll(counts).toEqual({ starts: ["default", "retry_failed", "regenerate"], saves: 1 });
  await expect(reader.locator('[data-block-id="7"] .block-tr')).toContainText("第 8 段");
  await push("allFailed");
  await expect(reader.getByText("翻译服务连接失败", { exact: true })).toBeVisible();
  await expect(reader.getByRole("button", { name: "重试翻译", exact: true }).first()).toBeVisible();
  await expect(reader.locator('[data-block-id="7"] .block-tr')).toContainText("第 8 段");
  await reader.locator(".drawer-body").evaluate(el => { el.scrollTop = 1700; });
  const beforeClose = await anchor();
  await reader.getByRole("button", { name: "返回", exact: true }).click();
  await open();
  await expect(rail.getByRole("radio", { name: "原文", checked: true })).toBeVisible();
  await expect(reader.getByText("翻译服务连接失败", { exact: true })).toBeVisible();
  expect((await anchor()).id).toBe(beforeClose.id);
  // 重开默认原文，短于原来的双语块时限制偏移，仍留在同一段。
  expect((await anchor()).offset).toBeLessThanOrEqual(0);
  await rail.getByRole("radio", { name: "双语", exact: true }).click();
  await expect(reader.locator('[data-block-id="7"] .block-tr')).toContainText("第 8 段");
  expect((await counts()).starts).toHaveLength(3);
  const width = await page.evaluate(() => ({ width: innerWidth, scroll: document.documentElement.scrollWidth }));
  expect(width.scroll).toBeLessThanOrEqual(width.width);
});
