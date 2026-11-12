import { aiPageFixture, bindingsFixture } from "./test/fixtures/settings";
import { act } from "react";
import { notifyManager } from "@tanstack/react-query";
import { afterAll, afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { api, type ArticleView, type DiagnosticsPage } from "./lib/api";
import { PREVIEW_DIGESTS, PREVIEW_FEEDS, PREVIEW_FOCUS } from "./lib/preview";
import { createReactHarness } from "./test/react";
import { click, clickText, pressKey, typeInto } from "./test/fixtures/shell";
import { focusRow, readerView, translationReply, translationSnapshot } from "./test/fixtures/reader";

// 这里只保留跨域装配路径；组件渲染和纯函数在所属模块直接测试。
const harness = createReactHarness();
const flush = harness.flush;
// 查询通知改用微任务，与有界 flush 对齐。
beforeAll(() => notifyManager.setScheduler(queueMicrotask));
afterAll(() => notifyManager.setScheduler((callback) => setTimeout(callback, 0)));
beforeEach(() => {
  vi.spyOn(api, "digests").mockImplementation(async (period) => PREVIEW_DIGESTS.filter((digest) => digest.period === period));
});
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });

async function openReader(host: HTMLElement) {
  clickText(host, "简报");
  await flush();
  click(host.querySelector(".digest-ref"));
  await flush();
  expect(host.querySelector(".reader-stage")).not.toBeNull();
}
async function search(host: HTMLElement, text: string) {
  clickText(host, "搜索");
  typeInto(host.querySelector<HTMLInputElement>(".command-palette-input")!, text);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 220)); });
  click([...host.querySelectorAll("button")].find((button) => button.textContent?.includes(text)));
  await flush();
}

it("keeps digest as a main view through Escape, settings and source reader navigation", async () => {
  const { host } = harness.mount(<App />);
  await flush();
  expect(host.querySelector(".composer")).toBeNull();
  clickText(host, "简报");
  await flush();
  expect(host.querySelector(".composer")).not.toBeNull();
  expect(host.querySelector(".overlay")).toBeNull();
  expect(host.querySelector(".digest")).not.toBeNull();
  expect(host.textContent).toContain("生成本期");
  pressKey("Escape");
  expect(host.querySelector(".digest")).not.toBeNull();
  clickText(host, "设置");
  await flush();
  expect(host.querySelector(".overlay")).not.toBeNull();
  clickText(host, "返回阅读");
  await flush();
  expect(host.querySelector(".overlay")).toBeNull();
  expect(host.querySelector(".digest")).not.toBeNull();
  expect(host.querySelector(".digest-ref")?.textContent).toBe("1");
  click(host.querySelector(".digest-ref"));
  await flush();
  expect(host.querySelector(".reader-stage")).not.toBeNull();
  expect(host.querySelector(".digest")).not.toBeNull();
  clickText(host, "返回");
  await flush();
  expect(host.querySelector(".reader-stage")).toBeNull();
  expect(host.querySelector("h3")?.textContent).toBe("今日三条主线");
});

it("keeps theme on the settings chrome when navigating diagnostics and models", async () => {
  const diagnostics: DiagnosticsPage = { vec: "就绪", stats: "测试数据", db: "临时库", ai: "未使用", log: null, scheduler: null };
  vi.spyOn(api, "diagnostics").mockResolvedValue(diagnostics);
  vi.spyOn(api, "ai").mockResolvedValue(aiPageFixture());
  vi.spyOn(api, "modelBindings").mockResolvedValue(bindingsFixture());
  vi.spyOn(api, "providerPresets").mockResolvedValue([]);
  const { host } = harness.mount(<App />);
  await flush();
  clickText(host, "设置");
  await flush();
  expect(host.textContent).toContain("外观");
  expect(host.textContent).toContain("跟随系统");
  act(() => host.querySelector<HTMLButtonElement>('[data-settings-tab="diagnostics"]')!.click());
  await flush();
  expect(host.textContent).toContain("索引维护");
  expect(host.textContent).toContain("处理一批");
  expect(host.textContent).not.toContain("主题模式");
  act(() => host.querySelector<HTMLButtonElement>('[data-settings-tab="model"]')!.click());
  await flush();
  expect(host.textContent).toContain("添加连接");
  expect(host.textContent).toContain("还没有连接");
  expect(host.textContent).toContain("跟随系统");
});

async function leaveSearchReader(host: HTMLElement) {
  click(host.querySelector(".reader-back"));
  await flush();
  expect(host.querySelector(".reader-stage")).toBeNull();
  const input = host.querySelector<HTMLInputElement>(".command-palette-input");
  expect(input).not.toBeNull();
  pressKey("Escape", {}, input!);
  await flush();
  expect(host.querySelector(".command-palette")).toBeNull();
}

it("stars a search-only article with S and refreshes the starred focus scope", async () => {
  const found = { ...focusRow(99, "搜索到的独立文章"), starred: true };
  let include = false;
  vi.spyOn(api, "feeds").mockResolvedValue(PREVIEW_FEEDS);
  vi.spyOn(api, "focus").mockImplementation(async () => ({ ...PREVIEW_FOCUS, list_scope: "starred", articles: include ? [found] : [] }));
  vi.spyOn(api, "starArticle").mockImplementation(async (_id, next) => { include = next; });
  vi.spyOn(api, "searchArticles").mockResolvedValue([{ id: 99, title: found.title, feed_title: "测试源", feed_icon_href: "", published_at: null, score: 0, snippet: "", read: false, starred: true, title_match: true, body_match: false }]);
  const { host } = harness.mount(<App />);
  await flush();
  expect(host.querySelector(".empty")?.textContent).toContain("还没有收藏");
  expect(host.textContent).not.toContain(found.title);
  await search(host, "独立文章");
  expect(host.querySelector(".reader-stage")).not.toBeNull();
  expect(host.textContent).toContain("预览");
  pressKey("s");
  await flush();
  expect(api.starArticle).toHaveBeenCalledWith(99, true);
  // 顶栏收藏是图标按钮：已收藏体现在名称与按下状态上。
  expect(host.querySelector("button[aria-label='取消收藏']")?.getAttribute("aria-pressed")).toBe("true");
  // 从搜索打开：返回先回到搜索，再关掉搜索回到列表。
  await leaveSearchReader(host);
  expect(vi.mocked(api.focus).mock.calls.length).toBeGreaterThanOrEqual(2);
  expect(host.querySelector(".empty")).toBeNull();
  expect(host.textContent).toContain("已收藏");
});

it("removes the final clustered favorite through the reader and returns to an empty list", async () => {
  const card = { ...focusRow(42, "簇内唯一收藏"), cluster_id: 10, starred: true };
  let items: ArticleView[] = [{ ...card, id: 10, row_kind: 1, title: "一组相近", cluster_label: "一组相近" }, card];
  vi.spyOn(api, "feeds").mockResolvedValue(PREVIEW_FEEDS);
  vi.spyOn(api, "focus").mockImplementation(async () => ({ ...PREVIEW_FOCUS, list_scope: "starred", articles: items }));
  vi.spyOn(api, "starArticle").mockImplementation(async () => { items = []; });
  vi.spyOn(api, "searchArticles").mockResolvedValue([{ id: 42, title: card.title, feed_title: card.feed_title, feed_icon_href: "", published_at: null, score: 10, snippet: "", read: false, starred: true, title_match: true, body_match: false }]);
  const { host } = harness.mount(<App />);
  await flush();
  expect([...host.querySelectorAll("button")].find((button) => button.textContent === "收藏")?.getAttribute("data-on")).toBe("1");
  await search(host, "簇内唯一");
  click(host.querySelector("button[aria-label='取消收藏']"));
  await flush();
  expect(api.starArticle).toHaveBeenCalledWith(42, false);
  await leaveSearchReader(host);
  expect(host.querySelector(".empty")?.textContent).toContain("还没有收藏");
  for (const text of ["一组相近", "簇内唯一收藏"]) expect(host.textContent).not.toContain(text);
});

it("traps settings focus, gives search priority and restores the opener", async () => {
  const { host } = harness.mount(<App />);
  await flush();
  const settings = [...host.querySelectorAll("button")].find((button) => button.textContent === "设置")!;
  settings.focus(); click(settings);
  await flush();
  const dialog = host.querySelector<HTMLElement>(".settings-drawer")!;
  const back = host.querySelector<HTMLButtonElement>("[data-settings-back]")!;
  expect(dialog.getAttribute("role")).toBe("dialog");
  expect(dialog.getAttribute("aria-modal")).toBe("true");
  expect(host.querySelector("#settings-title")?.textContent).toBe("设置");
  expect(host.querySelector("#settings-desc")).not.toBeNull();
  expect(document.activeElement).toBe(back);
  expect(host.querySelector(".sidebar")?.hasAttribute("inert")).toBe(true);
  pressKey("Tab", { shiftKey: true }, dialog);
  expect(dialog.contains(document.activeElement)).toBe(true);
  expect(document.activeElement).not.toBe(back);
  expect((document.activeElement as HTMLElement).closest(".sidebar")).toBeNull();
  back.focus();
  pressKey("k", { ctrlKey: true });
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
  const input = host.querySelector<HTMLInputElement>(".command-palette-input")!;
  expect(document.activeElement).toBe(input);
  expect(dialog.closest(".overlay")?.hasAttribute("inert")).toBe(true);
  act(() => back.focus());
  expect(document.activeElement).toBe(input);
  // Tab 在搜索浮层内循环（输入框、筛选），不回到被压住的设置。
  pressKey("Tab", {}, input);
  expect(host.querySelector(".command-palette")?.contains(document.activeElement)).toBe(true);
  pressKey("Tab", { shiftKey: true }, document.activeElement as HTMLElement);
  expect(document.activeElement).toBe(input);
  pressKey("Escape", {}, input);
  await flush();
  expect(host.querySelector(".command-palette")).toBeNull();
  expect(host.querySelector(".settings-drawer")).not.toBeNull();
  expect(document.activeElement).toBe(back);
  click(back);
  await flush();
  expect(document.activeElement).toBe(settings);
});

it("restores a canceled confirmation and falls back when its trigger unmounts", async () => {
  let empty = false;
  vi.spyOn(api, "feeds").mockImplementation(async () => empty ? { folders: [], uncategorized: [], folder_combo: [] } : PREVIEW_FEEDS);
  vi.spyOn(api, "unsubscribe").mockImplementation(async () => { empty = true; });
  vi.spyOn(api, "deleteFolder").mockImplementation(async () => { empty = true; });
  const { host } = harness.mount(<App />);
  await flush(); clickText(host, "设置"); await flush(); clickText(host, "编辑"); await flush();
  const remove = [...host.querySelectorAll("button")].find((button) => button.textContent === "删除")!;
  remove.focus(); click(remove); await flush();
  expect(host.querySelector("[role=alertdialog]")?.classList.contains("confirm-panel")).toBe(true);
  expect(document.activeElement).toBe(host.querySelector("[data-confirm-cancel]"));
  click(host.querySelector("[data-confirm-cancel]")); await flush();
  expect(host.querySelector("[role=alertdialog]")).toBeNull();
  expect(document.activeElement).toBe(remove);
  click(remove); await flush(); click(host.querySelector("[data-confirm-ok]")); await flush();
  expect(host.querySelector("[role=alertdialog]")).toBeNull();
  expect([...host.querySelectorAll("button")].find((button) => button.textContent === "删除")).toBeUndefined();
  expect(document.activeElement).toBe(host.querySelector("[data-settings-back]"));
});

it("suspends reader actions under settings and restores reader focus", async () => {
  const { host } = harness.mount(<App />);
  await flush(); await openReader(host);
  const star = host.querySelector<HTMLButtonElement>("button[aria-label='收藏']")!;
  star.focus();
  const starAction = vi.spyOn(api, "starArticle");
  const translate = vi.spyOn(api, "translate");
  clickText(host, "设置"); await flush();
  expect(host.querySelector(".reader-stage")?.hasAttribute("inert")).toBe(true);
  pressKey("s"); pressKey("t"); await flush();
  expect(starAction).not.toHaveBeenCalled();
  expect(translate).not.toHaveBeenCalled();
  click(host.querySelector("[data-settings-back]")); await flush();
  expect(host.querySelector(".settings-drawer")).toBeNull();
  expect(host.querySelector(".reader-stage")?.hasAttribute("inert")).toBe(false);
  expect(document.activeElement).toBe(star);
});

it("runs reader translation with T and lets its tool menu consume Escape first", async () => {
  // 预览不再伪造译文；翻译结果由显式合成数据提供。
  const clock = harness.useClock();
  vi.spyOn(api, "openArticle").mockImplementation(async (id) => readerView(id));
  vi.spyOn(api, "translateSnapshot").mockImplementation(async (id) => translationReply(readerView(id)));
  const translate = vi.spyOn(api, "translate").mockImplementation(async (id) => translationReply(readerView(id), translationSnapshot(readerView(id), {
    phase: "completed", save: "saved", units: ["done", "done"], title: "Translated title",
    blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Translated paragraph" }] } }],
  })));
  const { host } = harness.mount(<App />);
  await flush(); await openReader(host);
  pressKey("t"); await flush(); await clock.advance(100); await flush();
  expect(translate).toHaveBeenCalledOnce();
  expect(host.querySelector(".reader-view-rail")).not.toBeNull();
  const tools = host.querySelector<HTMLDetailsElement>(".reader-tools")!;
  act(() => { tools.open = true; });
  pressKey("Escape", {}, tools.querySelector("summary")!);
  expect(tools.open).toBe(false);
  expect(host.querySelector(".reader-stage")).not.toBeNull();
});
