import { act } from "react";
import { vi } from "vitest";
import type { FocusPage } from "../../lib/api";
import type { ApiStub } from "../api";
import { aiPageFixture, bindingsFixture, modelPoolFixture } from "./settings";
import { TWO_ARTICLE_PAGE, readerView, translationReply } from "./reader";
import { createSettingsSchema } from "../../settings/registry";

/** 根壳首屏的读取（焦点、订阅、会话，以及输入区模型按钮的池、连接与绑定）；列表虚拟化需要 jsdom 给出非零尺寸。 */
export function handleShellReads(stub: ApiStub, page: FocusPage = TWO_ARTICLE_PAGE) {
  handleReaderReads(stub);
  stub.handle("focus", () => page);
  stub.handle("feeds", () => ({ folders: [], uncategorized: [], folder_combo: [] }));
  stub.handle("threadList", () => ({ folders: [], threads: [], archived_count: 0 }));
  // 搜索浮层同时搜会话（N4 · 搜索对话）。
  stub.handle("searchThreads", () => []);
  // 输入区的模型按钮读模型池、连接与绑定（阶段 19-6）。
  stub.handle("modelPool", () => modelPoolFixture());
  stub.handle("ai", () => aiPageFixture());
  stub.handle("modelBindings", () => bindingsFixture());
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1024);
}

export function clickText(host: HTMLElement, text: string) {
  const btn = [...host.querySelectorAll("button")].find((b) => b.textContent === text);
  if (!btn) throw new Error(`button "${text}" not found`);
  act(() => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  });
}

export function click(el: Element | null | undefined) {
  if (!el) throw new Error("element not found");
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  });
}

export function pressKey(name: string, init: KeyboardEventInit = {}, target: EventTarget = window) {
  act(() => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init }));
  });
}

/** 受控 input / textarea 的赋值要走原生 setter，React 才会看到变化。 */
export function typeInto(el: HTMLInputElement | HTMLTextAreaElement, text: string) {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setValue = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  act(() => {
    setValue?.call(el, text);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}


export function handleReaderReads(stub: ApiStub) {
  stub.handle("readingSpeeds", () => ({ chinese_original: null, chinese_translated: null, english_original: null, english_translated: null }));
  stub.handle("settingsSchema", () => createSettingsSchema());
  stub.handle("translateSnapshot", id => translationReply(readerView(id)));
  stub.handle("translateAttach", () => {});
  stub.handle("translateDetach", () => {});
  // 正文显示约 400ms 后会只读预取下一篇。慢机器上叠层测试还停在阅读器里，这次请求不能算意外调用。
  stub.handle("reader", id => readerView(id));
  stub.handle("retryImage", (_articleId, url) => ({ src: url, media_id: null, href: "", width: null, height: null, error: "" }));
  // 挂载根壳时侧栏「订阅」项读未读总数，阅读器换篇后也会刷新一次。
  stub.handle("feedCounts", () => ({ total: 0, feeds: [], folders: [] }));
}
