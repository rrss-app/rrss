import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { handleReaderReads } from "../../test/fixtures/shell";
import { settingsKeys } from "../../settings/settingsQuery";
import { act, createRef, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../../test/api";
import { createEventStub, type EventStub } from "../../test/events";
import { ARTICLE_A, ARTICLE_B, TWO_ARTICLE_PAGE, focusRow, readerView, translationReply, translationSnapshot } from "../../test/fixtures/reader";
import { createReactHarness } from "../../test/react";
import { ReaderHost, type ReaderHostHandle, type ReaderHostProps } from "./ReaderHost";
import { WebLinksContext } from "../../lib/webLinks";
import { deferred } from "../../test/deferred";
import type { WebVerifyStateEvent } from "../../bindings";
import { READER_CACHE_SIZE } from "./readerCache";

const box = vi.hoisted(() => ({
  stub: null as ApiStub | null,
  events: null as EventStub | null,
}));

vi.mock("../../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../../lib/api")>("../../lib/api");
  const api = await import("../../test/api");
  const events = await import("../../test/events");
  return {
    ...actual,
    api: api.createApiProxy(() => box.stub),
    ...events.eventListenerExports(() => box.events),
  };
});

describe("ReaderHost standalone", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };

  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    box.stub = createApiStub();
    handleReaderReads(box.stub);
    box.events = createEventStub();
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
    expect(box.stub!.callCount("reader")).toBe(0);
  });

  function key(name: string) {
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
    });
  }

  /** 最小根壳：只维护「阅读器是否打开」，其余回调记录调用。 */
  function setup(paginated = false, extra: Array<ReturnType<typeof focusRow>> = [], webContext: React.ContextType<typeof WebLinksContext> = null) {
    let articles = paginated ? TWO_ARTICLE_PAGE.articles.slice(0, 1) : [...TWO_ARTICLE_PAGE.articles, ...extra];
    let hasMore = paginated;
    const nav = () => ({ articles, hasMore });
    const ref = createRef<ReaderHostHandle>();
    const calls = {
      onRequestOpen: vi.fn(),
      onClose: vi.fn(),
      onReadChange: vi.fn(),
      onToggleStar: vi.fn(async () => true),
      onRate: vi.fn(),
      onDislike: vi.fn(),
      onAsk: vi.fn(),
      onCite: vi.fn(),
      onContinueAsking: vi.fn(),
      onReturnToThread: vi.fn(),
      notify: vi.fn(),
    };
    let open = false;
    const props = (): ReaderHostProps => ({
      open,
      dismissible: open,
      shortcuts: open,
      inert: false,
      nav: { ...nav(), getCurrent: nav, loadMore: async () => { articles = TWO_ARTICLE_PAGE.articles; hasMore = false; } },
      ...calls,
    });
    let rerender: () => void = () => {};
    function Probe() {
      const [, tick] = useState(0);
      rerender = () => tick((n) => n + 1);
      return <ReaderHost ref={ref} {...props()} />;
    }
    calls.onRequestOpen.mockImplementation(() => { open = true; rerender(); });
    calls.onClose.mockImplementation(() => { open = false; rerender(); });
    const client = new QueryClient();
    const mounted = harness.mount(<QueryClientProvider client={client}><WebLinksContext.Provider value={webContext}><Probe /></WebLinksContext.Provider></QueryClientProvider>);
    return { ref, calls, client, host: mounted.host };
  }

  const title = (host: HTMLElement) => host.querySelector(".reader-title")?.textContent;
  const body = (host: HTMLElement) => host.querySelector<HTMLDivElement>(".drawer-body")!;
  const summary = (host: HTMLElement) => host.querySelector(".reader-summary")?.textContent ?? "";
  async function open(ref: React.RefObject<ReaderHostHandle | null>, id: number) {
    await act(async () => { await ref.current!.openArticle(id); });
    await harness.flush();
  }
  it("does not refresh a different article when window extraction returns late", async () => {
    const url = "https://web.example.test/verified";
    const pending = deferred<WebVerifyStateEvent | null>();
    const extract = vi.fn(() => pending.promise);
    box.stub!.handle("openArticle", id => readerView(id, id === ARTICLE_A ? { url, web_extracted_at: 1000 } : {}));
    const { ref, host } = setup(false, [], { open: vi.fn(), menu: vi.fn(), hasWindow: () => true, extract });
    await open(ref, ARTICLE_A);
    act(() => [...host.querySelectorAll("button")].find(button => button.textContent === "重新抽取")!.click());
    expect(extract).toHaveBeenCalledWith(url);
    await open(ref, ARTICLE_B);
    act(() => pending.resolve({ label: "web-1", url, state: "Extracted", article_id: ARTICLE_A, title: "迟到正文", text_chars: 1000, image_count: 0, reason: null }));
    await harness.flush();
    expect(title(host)).toBe("乙文");
    expect(host.querySelector(".web-origin")).toBeNull();
    expect(box.stub!.callCount("reader")).toBe(0);
  });

  function measureBlocks() {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const top = this.dataset.blockId === undefined ? 0 : 400 + Number(this.dataset.blockId) * 200 - (this.closest(".drawer-body")?.scrollTop ?? 0);
      return { top, bottom: top + 200, left: 0, right: 640, width: 640, height: 200, x: 0, y: top, toJSON: () => ({}) };
    });
    return { v: 1, blocks: Array.from({ length: 12 }, (_, i) => ({ t: "p", inlines: [{ t: "t", text: `段落 ${i}` }] })) };
  }

  it("returning during web extraction isolates its late result", async () => {
    box.stub!.handle("openArticle", id => readerView(id));
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    const pending = box.stub!.deferNext("openWebLink");
    act(() => { void ref.current!.openWebLink("https://web.example.test/story"); });
    expect(title(host)).toBe("甲文");
    act(() => { expect(ref.current!.back()).toBe(true); });
    pending.resolve({ article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: "https://web.example.test/story", layer: "General" as const, text_chars: 100, image_count: 0 });
    await harness.flush();
    expect(title(host)).toBe("甲文");
    expect(box.stub!.callCount("openArticle")).toBe(1);
  });

  it("closing during web extraction isolates its late failure", async () => {
    const { ref, host, calls } = setup();
    const pending = box.stub!.deferNext("openWebLink");
    act(() => { void ref.current!.openWebLink("https://web.example.test/story"); });
    act(() => { ref.current!.close(); });
    await harness.flush();
    pending.reject({ kind: "Network", message: "offline" });
    await harness.flush();
    expect(calls.onRequestOpen).not.toHaveBeenCalled();
    expect(host.querySelector(".web-failure")).toBeNull();
  });

  it("a late collection updates the saved web page in the return stack", async () => {
    box.stub!.handle("openArticle", id => readerView(id));
    box.stub!.handle("openWebLink", url => ({ article_id: url.endsWith("/one") ? 10 : 11, temporary: true,
      actual_host: "web.example.test", actual_url: "https://web.example.test/story", layer: "General" as const, text_chars: 100, image_count: 0 }));
    const { ref, host } = setup();
    await act(async () => { await ref.current!.openWebLink("https://web.example.test/one"); });
    const pending = box.stub!.deferNext("collectArticle");
    act(() => { host.querySelector<HTMLButtonElement>('[aria-label="收录"]')!.click(); });
    await act(async () => { await ref.current!.openWebLink("https://web.example.test/two"); });
    pending.resolve();
    await harness.flush();
    act(() => { expect(ref.current!.back()).toBe(true); });
    expect(host.querySelector<HTMLButtonElement>('[aria-label="已收录"]')?.disabled).toBe(true);
    expect(host.querySelector(".web-origin")?.textContent).not.toContain("临时打开");
  });

  it("shows cached content and position before open returns, then reconciles in place and syncs read once", async () => {
    const document = measureBlocks();
    box.stub!.handle("openArticle", id => readerView(id, { document }));
    const { ref, calls, host } = setup();
    await open(ref, ARTICLE_A);
    body(host).scrollTop = 1800;
    await open(ref, ARTICLE_B);
    calls.onReadChange.mockClear(); calls.onRequestOpen.mockClear();
    const count = box.stub!.callCount("openArticle");
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    expect(title(host)).toBe("甲文");
    expect(body(host).scrollTop).toBe(1800);
    expect(box.stub!.callCount("openArticle")).toBe(count + 1);
    expect(calls.onReadChange).not.toHaveBeenCalled();
    const element = body(host);
    pending.resolve(readerView(ARTICLE_A, { document, summary: "核对后的摘要" }));
    await harness.flush();
    expect(summary(host)).toContain("核对后的摘要");
    expect(body(host)).toBe(element);
    expect(body(host).scrollTop).toBe(1800);
    expect(calls.onReadChange.mock.calls).toEqual([[ARTICLE_A, true]]);
    expect(calls.onRequestOpen).toHaveBeenCalledOnce();
    vi.restoreAllMocks();
  });

  it("resets to the top on a changed source without replaying the opening citation anchor", async () => {
    const document = measureBlocks();
    let current = readerView(ARTICLE_A, { document });
    box.stub!.handle("openArticle", id => id === ARTICLE_A ? current : readerView(id));
    box.stub!.handle("translateSnapshot", id => translationReply(id === ARTICLE_A ? current : readerView(id)));
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    await open(ref, ARTICLE_B);
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A, { anchor: { blockId: 8 }, citationSource: { kind: "thread", threadId: 9, title: "缓存会话" } }); });
    await harness.flush();
    const previous = body(host);
    expect(previous.scrollTop).toBeGreaterThan(1000);
    expect(host.querySelector(".reader-block-highlight")).not.toBeNull();
    current = { ...current, source_hash: "changed" };
    pending.resolve(current);
    await harness.flush();
    expect(body(host)).not.toBe(previous);
    expect(body(host).scrollTop).toBe(0);
    expect(host.querySelector(".reader-block-highlight")).toBeNull();
    expect(host.querySelector('[aria-label="对话"]')).not.toBeNull();
    vi.restoreAllMocks();
  });

  it.each([false, true])("keeps a late open only in the cache after navigation / close (%s)", async closed => {
    box.stub!.handle("openArticle", id => readerView(id));
    const { ref, calls, host } = setup();
    await open(ref, ARTICLE_A);
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await open(ref, ARTICLE_B);
    if (closed) act(() => ref.current!.close());
    const count = calls.onRequestOpen.mock.calls.length;
    pending.resolve(readerView(ARTICLE_A, { summary: "迟到的甲文快照" }));
    await harness.flush();
    expect(title(host)).toBe(closed ? undefined : "乙文");
    expect(calls.onRequestOpen).toHaveBeenCalledTimes(count);
    const next = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    expect(summary(host)).toContain("迟到的甲文快照");
    next.resolve(readerView(ARTICLE_A));
    await harness.flush();
  });

  it("does not let an older same-article response replace a newer cached snapshot", async () => {
    box.stub!.handle("openArticle", id => readerView(id));
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    const old = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await open(ref, ARTICLE_B);
    const latest = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    latest.resolve(readerView(ARTICLE_A, { summary: "最新快照" }));
    await harness.flush();
    old.resolve(readerView(ARTICLE_A, { summary: "过期快照" }));
    await harness.flush();
    expect(summary(host)).toContain("最新快照");
    await open(ref, ARTICLE_B);
    box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    expect(summary(host)).toContain("最新快照");
  });

  it("does not overwrite a late snapshot with the old displayed session while another article is loading", async () => {
    box.stub!.handle("openArticle", id => readerView(id));
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    const pendingA = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    const pendingB = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_B); });
    pendingA.resolve(readerView(ARTICLE_A, { summary: "迟到的新快照" }));
    await harness.flush();
    // 乙文尚未返回，旧甲文还在屏幕上。配图只合入缓存自己的最新数据。
    act(() => box.events!.emit("media:ready", { article_id: ARTICLE_A, media: { src: "photo.png", href: "rrss://media/photo", width: null, height: null, error: "" } }));
    await harness.flush();
    expect(summary(host)).not.toContain("迟到的新快照");
    pendingB.resolve(readerView(ARTICLE_B));
    await harness.flush();
    box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    expect(summary(host)).toContain("迟到的新快照");
  });

  it("evicts the least recently used article when the ninth is opened", async () => {
    box.stub!.handle("openArticle", id => readerView(id, { title: `文章 ${id}` }));
    const { ref, host } = setup();
    expect(READER_CACHE_SIZE).toBe(8);
    for (let id = 1; id <= READER_CACHE_SIZE; id++) await open(ref, id);
    await open(ref, 1); // 使用，而非响应到达时间决定 LRU。
    await open(ref, 9);
    const hit = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(1); });
    await harness.flush();
    expect(title(host)).toBe("文章 1");
    const miss = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(2); });
    await harness.flush();
    expect(title(host)).toBe("文章 1");
    miss.resolve(readerView(2, { title: "重新读取文章 2" }));
    hit.resolve(readerView(1));
    await harness.flush();
    expect(title(host)).toBe("重新读取文章 2");
  });

  it("keeps cached content on open failure, reports once, and does not sync read", async () => {
    box.stub!.handle("openArticle", id => readerView(id));
    const { ref, calls, host } = setup();
    await open(ref, ARTICLE_A);
    act(() => ref.current!.close());
    calls.onReadChange.mockClear();
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    pending.reject(new Error("offline"));
    await harness.flush();
    expect(title(host)).toBe("甲文");
    expect(host.textContent).toContain("正文 1");
    expect(calls.notify.mock.calls).toEqual([[{ message: "打开失败：offline" }]]);
    expect(calls.onReadChange).not.toHaveBeenCalled();
  });

  it.each(["fulltext", "summary", "translation", "star", "feedback", "media", "cover"] as const)("writes %s updates through to the next cached open", async kind => {
    const clock = harness.useClock();
    let current = readerView(ARTICLE_A, { truncated: true });
    box.stub!.handle("openArticle", id => id === ARTICLE_A ? current : readerView(id));
    box.stub!.handle("translateSnapshot", id => translationReply(id === ARTICLE_A ? current : readerView(id)));
    const { ref, calls, host } = setup();
    await open(ref, ARTICLE_A);
    if (kind === "fulltext") {
      box.stub!.handle("fetchFulltext", () => current = { ...current, source_hash: "fulltext", truncated: false,
        document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "新的完整正文" }] }] } });
      key("u");
    } else if (kind === "summary") {
      box.stub!.handle("summarize", () => "新生成摘要");
      act(() => host.querySelector<HTMLButtonElement>('[aria-label="生成本篇摘要"]')!.click());
    } else if (kind === "translation") {
      box.stub!.handle("translate", () => translationReply(current, translationSnapshot(current, {
        phase: "completed", save: "saved", units: ["done", "done"], title: "Cached translation",
        blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Translated body" }] } }],
      })));
      key("t");
    } else if (kind === "star") key("s");
    else if (kind === "feedback") {
      calls.onRate.mockResolvedValue(true);
      act(() => host.querySelector<HTMLButtonElement>('.reader-ending-actions button')!.click());
    } else if (kind === "media") {
      current = { ...current, document: { v: 1, blocks: [{ t: "img", src: "photo.png", alt: "图片" }] } };
      box.stub!.handle("fetchFulltext", () => current);
      key("u"); await harness.flush();
      act(() => box.events!.emit("media:ready", { article_id: ARTICLE_A, media: { src: "photo.png", href: "rrss://media/photo", width: null, height: null, error: "" } }));
    } else {
      current = { ...current, document: { v: 1, blocks: [{ t: "embed", url: "https://www.youtube.com/watch?v=example", kind: "youtube" }] } };
      box.stub!.handle("fetchFulltext", () => current);
      key("u"); await harness.flush();
      act(() => box.events!.emit("embed:cover", { article_id: ARTICLE_A, cover: { url: "https://www.youtube.com/watch?v=example", open_url: "https://www.youtube.com/watch?v=example", cover_href: "rrss://media/cover", failed: false, title: "视频", site: "youtube", author: "", duration_secs: null } }));
    }
    await harness.flush(); await clock.advance(100);
    await open(ref, ARTICLE_B);
    box.stub!.deferNext("openArticle");
    box.stub!.deferNext("translateSnapshot");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    if (kind === "fulltext") expect(host.textContent).toContain("新的完整正文");
    if (kind === "summary") expect(summary(host)).toContain("新生成摘要");
    if (kind === "translation") { key("v"); expect(host.textContent).toContain("Translated body"); }
    if (kind === "star") expect(host.querySelector('[aria-label="取消收藏"]')).not.toBeNull();
    if (kind === "feedback") expect(host.querySelector('.reader-ending-actions button')?.getAttribute("aria-pressed")).toBe("true");
    if (kind === "media") expect(host.querySelector('img[src="rrss://media/photo"]')).not.toBeNull();
    if (kind === "cover") expect(host.querySelector('img[src="rrss://media/cover"]')).not.toBeNull();
  });

  it("protects edits made while a cached open is being checked", async () => {
    box.stub!.handle("openArticle", id => readerView(id));
    box.stub!.handle("summarize", () => "等待时写的摘要");
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    await open(ref, ARTICLE_B);
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    key("s");
    act(() => host.querySelector<HTMLButtonElement>('[aria-label="生成本篇摘要"]')!.click());
    await harness.flush();
    pending.resolve(readerView(ARTICLE_A, { summary: "旧摘要" }));
    await harness.flush();
    expect(summary(host)).toContain("等待时写的摘要");
    expect(host.querySelector('[aria-label="取消收藏"]')).not.toBeNull();
  });

  it("does not undo fulltext replacement when the cached open returns its old body", async () => {
    let current = readerView(ARTICLE_A, { truncated: true });
    box.stub!.handle("openArticle", id => id === ARTICLE_A ? current : readerView(id));
    box.stub!.handle("translateSnapshot", id => translationReply(id === ARTICLE_A ? current : readerView(id)));
    box.stub!.handle("fetchFulltext", () => current = { ...current, source_hash: "replacement", truncated: false,
      document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "替换的全文" }] }] } });
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    await open(ref, ARTICLE_B);
    const old = current;
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    key("u"); await harness.flush();
    const element = body(host);
    pending.resolve(old);
    await harness.flush();
    expect(body(host)).toBe(element);
    expect(host.textContent).toContain("替换的全文");
  });

  it("keeps hostless preview outside the cache", async () => {
    delete win.__TAURI_INTERNALS__;
    box.stub!.handle("openArticle", () => { throw new Error("no host"); });
    const { ref, calls, host } = setup();
    await open(ref, ARTICLE_A);
    expect(title(host)).toBe("甲文");
    act(() => ref.current!.close());
    calls.onRequestOpen.mockClear();
    const pending = box.stub!.deferNext("openArticle");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    expect(title(host)).toBeUndefined();
    expect(calls.onRequestOpen).not.toHaveBeenCalled();
    pending.reject(new Error("no host"));
    await harness.flush();
    expect(title(host)).toBe("甲文");
    expect(box.stub!.callCount("translateSnapshot")).toBe(0);
  });

  it("rechecks and attaches translation even when the cached article already has a completed translation", async () => {
    const clock = harness.useClock();
    const current = readerView(ARTICLE_A, { translation_ready: true, translated_title: "Old title" });
    box.stub!.handle("openArticle", id => id === ARTICLE_A ? current : readerView(id));
    box.stub!.handle("translateSnapshot", id => translationReply(id === ARTICLE_A ? current : readerView(id)));
    const { ref, host } = setup();
    await open(ref, ARTICLE_A);
    await open(ref, ARTICLE_B);
    const count = box.stub!.callCount("translateSnapshot");
    box.stub!.deferNext("openArticle");
    const snapshot = box.stub!.deferNext("translateSnapshot");
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    expect(box.stub!.callCount("translateSnapshot")).toBe(count + 1);
    snapshot.resolve(translationReply(current, translationSnapshot(current, {
      phase: "completed", save: "saved", units: ["done", "done"], title: "Live translation",
      blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Latest translation" }] } }],
    })));
    await harness.flush(); await clock.advance(100);
    expect(box.stub!.callCount("translateAttach")).toBe(1);
    key("v");
    expect(title(host)).toBe("Live translation");
    expect(box.stub!.callCount("translate")).toBe(0);
  });

  it("opens through the handle, syncs read state, and handles m / v / s shortcuts itself", async () => {
    box.stub!.handle("openArticle", (id: number) =>
      readerView(id, id === ARTICLE_A ? { translation_ready: true, translated_title: "Article A" } : {}),
    );
    box.stub!.handle("translateSnapshot", id => translationReply(readerView(id, id === ARTICLE_A ? { translation_ready: true, translated_title: "Article A" } : {})));
    const marked: Array<[number, boolean]> = [];
    box.stub!.handle("markRead", (id: number, read: boolean) => { marked.push([id, read]); });
    const { ref, calls, host } = setup(true);

    await act(async () => { await ref.current!.openArticle(ARTICLE_A, { returnToList: true }); });
    await harness.flush();
    expect(calls.onReadChange).toHaveBeenCalledWith(ARTICLE_A, true);
    expect(calls.onRequestOpen).toHaveBeenCalledOnce();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
    expect(host.querySelector<HTMLButtonElement>('[aria-label="下一篇"]')?.disabled).toBe(false);

    key("v");
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("Article A");
    expect(host.querySelector('.reader-view-rail [aria-checked="true"]')?.getAttribute("aria-label")).toBe("双语");

    key("m");
    await harness.flush();
    expect(marked).toEqual([[ARTICLE_A, false]]);
    expect(calls.onReadChange).toHaveBeenLastCalledWith(ARTICLE_A, false);

    key("s");
    await harness.flush();
    expect(calls.onToggleStar).toHaveBeenCalledWith(ARTICLE_A, false);
    expect(host.querySelector('button[aria-label="取消收藏"]')).not.toBeNull();

    key("j");
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("乙文");
    expect(ref.current!.getReturnArticleId()).toBe(ARTICLE_B);
    expect(host.querySelector('.reader-view-rail')).toBeNull();
    key("k");
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
  });

  it("commits one article's read-state writes in the order the user made them", async () => {
    // 宿主可能反序完成：打开在完成时标已读，M 在完成时写未读。
    const read = new Map<number, boolean>();
    let openGate: Promise<void> | null = null;
    let markGate: Promise<void> | null = null;
    box.stub!.handle("openArticle", async (id: number) => { if (openGate) await openGate; read.set(id, true); return readerView(id); });
    box.stub!.handle("markRead", async (id: number, value: boolean) => { if (markGate) await markGate; read.set(id, value); });
    const { ref, calls } = setup();
    await open(ref, ARTICLE_A);
    await open(ref, ARTICLE_B);
    const opening = deferred<void>();
    openGate = opening.promise;
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    key("m"); await harness.flush();
    opening.resolve(); await harness.flush();
    expect(read.get(ARTICLE_A)).toBe(false);
    expect(calls.onReadChange).toHaveBeenLastCalledWith(ARTICLE_A, false);
    openGate = null;
    const marking = deferred<void>();
    markGate = marking.promise;
    key("m"); await harness.flush();
    act(() => { void ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    marking.resolve(); await harness.flush();
    expect(read.get(ARTICLE_A)).toBe(true);
    expect(calls.onReadChange).toHaveBeenLastCalledWith(ARTICLE_A, true);
  });

  it("keeps the conversation return only until a different article is opened", async () => {
    box.stub!.handle("openArticle", (id: number) => readerView(id));
    const { ref, calls, host } = setup();
    await act(async () => { await ref.current!.openArticle(ARTICLE_A, { citationSource: { kind: "thread", threadId: 9, title: "原会话" } }); });
    await harness.flush();
    expect(ref.current!.getSourceArticle()).toEqual({ id: ARTICLE_A, title: "甲文" });
    key("q");
    expect(calls.onContinueAsking).toHaveBeenCalledOnce();
    // 同篇重新打开不等于换篇；关闭句柄用于根壳 Esc / Backspace。
    await act(async () => { await ref.current!.openArticle(ARTICLE_A); });
    act(() => ref.current!.close());
    expect(calls.onReturnToThread).toHaveBeenCalledWith(9);
    await act(async () => { await ref.current!.openArticle(ARTICLE_A, { citationSource: { kind: "thread", threadId: 9, title: "原会话" } }); });
    key("j"); await harness.flush();
    key("k"); await harness.flush();
    key("q");
    expect(calls.onContinueAsking).toHaveBeenCalledOnce();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
    act(() => ref.current!.close());
    expect(calls.onReturnToThread).toHaveBeenCalledTimes(1);
    expect(ref.current!.getSourceArticle()).toBeNull();
  });

  it("does not replay a citation anchor after fulltext changes the body version", async () => {
    const document = { v: 1, blocks: Array.from({ length: 3 }, () => ({ t: "p", inlines: [{ t: "t", text: "正文" }] })) };
    const view = readerView(ARTICLE_A, { truncated: true, document });
    box.stub!.handle("openArticle", () => view);
    box.stub!.handle("translateSnapshot", () => translationReply(view));
    box.stub!.handle("fetchFulltext", () => ({ ...view, source_hash: "new-body", truncated: false }));
    const { ref, host } = setup();
    await act(async () => { await ref.current!.openArticle(ARTICLE_A, { anchor: { blockId: 2 } }); });
    await harness.flush();
    expect(host.querySelector(".reader-block-highlight")).not.toBeNull();
    key("u"); await harness.flush();
    expect(host.querySelector(".reader-block-highlight")).toBeNull();
  });

  it("keeps T translation and U fulltext shortcuts with their original eligibility", async () => {
    const view = readerView(ARTICLE_A, { truncated: true });
    box.stub!.handle("openArticle", () => view);
    box.stub!.handle("translateSnapshot", () => translationReply(view));
    const translate = vi.fn(() => translationReply(view));
    const fulltext = vi.fn(() => ({ ...view, truncated: false }));
    box.stub!.handle("translate", translate);
    box.stub!.handle("fetchFulltext", fulltext);
    const { ref } = setup();
    await act(async () => { await ref.current!.openArticle(ARTICLE_A); });
    await harness.flush();
    key("t");
    await harness.flush();
    expect(translate).toHaveBeenCalledWith(ARTICLE_A, "default", null);
    key("u");
    await harness.flush();
    expect(fulltext).toHaveBeenCalledWith(ARTICLE_A);
    key("u");
    await harness.flush();
    expect(box.stub!.callCount("fetchFulltext")).toBe(1);
  });

  it("drops every in-flight result once the shell closes the reader", async () => {
    const gates: Array<{ id: number; resolve: (v: ReturnType<typeof readerView>) => void }> = [];
    box.stub!.handle("openArticle", (id: number) => {
      const pending = deferred<ReturnType<typeof readerView>>();
      gates.push({ id, resolve: pending.resolve });
      return pending.promise;
    });
    const { ref, calls, host } = setup();

    void ref.current!.openArticle(ARTICLE_A);
    await harness.flush();
    gates[0].resolve(readerView(ARTICLE_A));
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");

    key("j");
    await harness.flush();
    expect(gates.map((g) => g.id)).toEqual([ARTICLE_A, ARTICLE_B]);
    act(() => calls.onClose());
    await harness.flush();
    expect(host.querySelector(".reader-stage")).toBeNull();

    gates[1].resolve(readerView(ARTICLE_B));
    await harness.flush();
    expect(calls.onRequestOpen).toHaveBeenCalledOnce();
    expect(calls.onReadChange).toHaveBeenCalledWith(ARTICLE_B, true);
    expect(host.querySelector(".reader-stage")).toBeNull();
  });

  it("opens the article drawer and rates the current row before moving forward", async () => {
    box.stub!.handle("openArticle", (id: number) => readerView(id));
    const { ref, calls, host } = setup();
    await act(async () => { await ref.current!.openArticle(ARTICLE_A, { returnToList: true }); });
    await harness.flush();
    key("l");
    expect(host.querySelector(".reader-article-drawer")).not.toBeNull();
    key("x");
    await harness.flush();
    expect(calls.onDislike).toHaveBeenCalledWith(expect.objectContaining({ id: ARTICLE_A }));
    expect(calls.onRate).not.toHaveBeenCalled();
    expect(host.querySelector(".reader-title")?.textContent).toBe("乙文");
    expect(host.querySelector('[data-reader-drawer-row="1"]')).toBeNull();
  });

  it("rates another drawer row without leaving the current article", async () => {
    box.stub!.handle("openArticle", (id: number) => readerView(id));
    const { ref, calls, host } = setup(false, [focusRow(3, "丙文")]);
    await act(async () => { await ref.current!.openArticle(ARTICLE_A, { returnToList: true }); });
    await harness.flush();
    key("l");
    key("ArrowDown");
    key("x");
    await harness.flush();
    expect(calls.onDislike).toHaveBeenCalledWith(expect.objectContaining({ id: ARTICLE_B, title: "乙文", feed_title: "示例信源" }));
    expect(calls.onRate).not.toHaveBeenCalled();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
    expect(host.querySelector('[data-reader-drawer-row="2"]')).toBeNull();
    expect(host.querySelector(".reader-article-drawer")).not.toBeNull();
    expect(host.querySelector(".reader-article-drawer-row.selected")?.getAttribute("data-reader-drawer-row")).toBe("3");
    expect(host.querySelector('[data-reader-drawer-row="1"] [aria-current="true"]')).not.toBeNull();
  });

  it("rechecks saved translation after settings change without starting generation", async () => {
    const clock = harness.useClock();
    const cached = readerView(ARTICLE_A, { translation_ready: true, translated_title: "Article A" });
    box.stub!.handle("openArticle", () => cached);
    box.stub!.handle("translateSnapshot", () => translationReply(cached, translationSnapshot(cached, {
      phase: "completed", save: "saved", units: ["done", "done"], title: "Article A",
      blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Translated paragraph" }] } }],
    })));
    const { ref, client, host } = setup();
    await act(async () => { await ref.current!.openArticle(ARTICLE_A); });
    await clock.advance(100);
    await harness.flush();
    expect(host.querySelector(".reader-view-rail")).not.toBeNull();
    const range = document.createRange();
    range.selectNodeContents(host.querySelector("[data-reader-document] p")!);
    window.getSelection()!.addRange(range);
    const calls = box.stub!.callCount("translateSnapshot");
    box.stub!.handle("translateSnapshot", () => translationReply(readerView(ARTICLE_A)));
    act(() => client.setQueryData(settingsKeys.schema, client.getQueryData(settingsKeys.schema), {
      updatedAt: client.getQueryState(settingsKeys.schema)!.dataUpdatedAt + 1,
    }));
    await clock.advance(100);
    await harness.flush();
    expect(box.stub!.callCount("translateSnapshot")).toBe(calls + 1);
    expect(host.querySelector(".reader-view-rail")).toBeNull();
    expect(box.stub!.callCount("translate")).toBe(0);
    window.getSelection()!.removeAllRanges();
  });
});
