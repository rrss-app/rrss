import type { TranslationReply } from "../bindings";
import { translationReply, translationSnapshot } from "./fixtures/reader";
import { handleReaderReads } from "./fixtures/shell";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import type { ReaderView } from "../lib/api";
import { createApiStub, type ApiStub } from "./api";
import { deferred, type Deferred } from "./deferred";
import { createEventStub, type EventStub } from "./events";
import { ARTICLE_A, ARTICLE_B, TWO_ARTICLE_PAGE, readerView } from "./fixtures/reader";
import { createReactHarness } from "./react";

const box = vi.hoisted(() => ({
  stub: null as ApiStub | null,
  events: null as EventStub | null,
}));

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  const api = await import("./api");
  const events = await import("./events");
  return {
    ...actual,
    api: api.createApiProxy(() => box.stub),
    ...events.eventListenerExports(() => box.events),
  };
});

const A = ARTICLE_A;
const B = ARTICLE_B;
const PAGE = TWO_ARTICLE_PAGE;
const view = readerView;

type Pending<T> = { id: number; gate: Deferred<T> };

/** 每次调用单独一个 Deferred，按调用顺序记录，测试决定谁先返回。 */
function gated<T>(name: "openArticle" | "summarize" | "translate" | "fetchFulltext" | "prefetch") {
  const calls: Pending<T>[] = [];
  box.stub!.handle(name, ((id: number) => {
    const gate = deferred<T>();
    calls.push({ id, gate });
    return gate.promise;
  }) as never);
  return calls;
}

describe("reader request ownership", () => {
  const harness = createReactHarness();
  let readers: Pending<ReaderView>[];

  const win = window as Window & { __TAURI_INTERNALS__?: unknown };

  beforeEach(() => {
    // 走宿主路径：失败必须报错，而不是回退到预览样例。
    win.__TAURI_INTERNALS__ = {};
    box.stub = createApiStub();
    handleReaderReads(box.stub);
    box.events = createEventStub();
    box.stub.handle("focus", () => PAGE);
    box.stub.handle("feeds", () => ({ folders: [], uncategorized: [], folder_combo: [] }));
    box.stub.handle("threadList", () => ({ folders: [], threads: [], archived_count: 0 }));
    readers = gated<ReaderView>("openArticle");
    // 固定时钟：启动 800ms 后的设置预载不在本测试范围内。
    harness.useClock();
    // jsdom 没有布局，虚拟列表按 offsetHeight 决定视口，给一个非零高度才会渲染行。
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1024);
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
  });

  async function mountHome() {
    const mounted = harness.mount(<App />);
    await harness.flush();
    expect(mounted.host.textContent).toContain("甲文");
    return mounted;
  }

  function click(el: Element | null | undefined) {
    expect(el).toBeTruthy();
    act(() => {
      (el as HTMLElement).click();
    });
  }

  function openFromList(host: HTMLElement, title: string) {
    const card = [...host.querySelectorAll("article")].find((a) => a.textContent?.includes(title));
    expect(card).toBeTruthy();
    act(() => {
      card!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
  }

  function readerTitle(host: HTMLElement) {
    return host.querySelector(".reader-title")?.textContent ?? null;
  }

  function stage(host: HTMLElement) {
    return host.querySelector(".reader-stage");
  }

  function progress(host: HTMLElement) {
    return host.querySelector('.reader-summary[aria-busy="true"] .reader-summary-label, .reader-progress')?.textContent ?? "";
  }

  function readerAction(host: HTMLElement, label: string) {
    return [...host.querySelectorAll<HTMLButtonElement>(".reader-head button")].find((b) =>
      (b.getAttribute("aria-label") ?? b.textContent)?.includes(label),
    );
  }

  function navButton(host: HTMLElement, label: "上一篇" | "下一篇") {
    return host.querySelector<HTMLButtonElement>(`.reader-pager button[aria-label="${label}"]`);
  }

  function backButton(host: HTMLElement) {
    return [...host.querySelectorAll<HTMLButtonElement>(".reader-head-nav button")].find((b) => b.textContent === "返回");
  }

  async function openResolved(host: HTMLElement, id: number, extra: Partial<ReaderView> = {}) {
    openFromList(host, id === A ? "甲文" : "乙文");
    await harness.flush();
    const call = readers.at(-1)!;
    expect(call.id).toBe(id);
    call.gate.resolve(view(id, extra));
    await harness.flush();
    expect(readerTitle(host)).toBe(id === A ? "甲文" : "乙文");
  }

  it("fast A→B shows B even when A resolves last", async () => {
    const { host } = await mountHome();
    openFromList(host, "甲文");
    await harness.flush();
    openFromList(host, "乙文");
    await harness.flush();
    expect(readers.map((c) => c.id)).toEqual([A, B]);

    readers[1].gate.resolve(view(B));
    await harness.flush();
    expect(readerTitle(host)).toBe("乙文");

    readers[0].gate.resolve(view(A));
    await harness.flush();
    expect(readerTitle(host)).toBe("乙文");
    expect(host.querySelectorAll(".reader-stage")).toHaveLength(1);
  });

  it("A→B→A keeps the latest A session, not the first A response", async () => {
    const { host } = await mountHome();
    openFromList(host, "甲文");
    await harness.flush();
    openFromList(host, "乙文");
    await harness.flush();
    openFromList(host, "甲文");
    await harness.flush();
    expect(readers.map((c) => c.id)).toEqual([A, B, A]);

    readers[2].gate.resolve(view(A, { summary: "第三次打开" }));
    await harness.flush();
    expect(readerTitle(host)).toBe("甲文");
    expect(host.querySelector(".reader-summary")?.textContent).toContain("第三次打开");

    readers[0].gate.resolve(view(A, { summary: "第一次打开" }));
    readers[1].gate.resolve(view(B));
    await harness.flush();
    expect(readerTitle(host)).toBe("甲文");
    expect(host.querySelector(".reader-summary")?.textContent).toContain("第三次打开");
  });

  it("a response arriving after close does not reopen the reader", async () => {
    const { host } = await mountHome();
    await openResolved(host, A);
    click(navButton(host, "下一篇"));
    await harness.flush();
    expect(readers.at(-1)?.id).toBe(B);

    click(backButton(host));
    await harness.flush();
    expect(stage(host)).toBeNull();
    const depth = history.length;

    readers.at(-1)!.gate.resolve(view(B));
    await harness.flush();
    expect(stage(host)).toBeNull();
    expect(history.length).toBe(depth);

    // 关闭后再打开同一篇是新的会话，正常显示。
    await openResolved(host, B);
    expect(stage(host)).not.toBeNull();
  });

  it("late summary / translation / fulltext for A never touch B", async () => {
    const summaries = gated<string>("summarize");
    const translations = gated<TranslationReply>("translate");
    const fulltexts = gated<ReaderView>("fetchFulltext");
    const { host } = await mountHome();
    await openResolved(host, A, { truncated: true });

    click(readerAction(host, "生成本篇摘要"));
    click(readerAction(host, "翻译"));
    click(readerAction(host, "抓取全文"));
    await harness.flush();
    expect(summaries).toHaveLength(1);
    expect(translations).toHaveLength(1);
    expect(fulltexts).toHaveLength(1);
    expect(progress(host)).not.toBe("");

    click(navButton(host, "下一篇"));
    await harness.flush();
    readers.at(-1)!.gate.resolve(view(B));
    await harness.flush();
    expect(readerTitle(host)).toBe("乙文");
    expect(progress(host)).toBe("");

    summaries[0].gate.resolve("甲文摘要");
    translations[0].gate.resolve(translationReply(view(A), translationSnapshot(view(A), { phase: "completed", save: "saved", units: ["done", "done"], title: "Article A", blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Translated" }] } }] })));
    fulltexts[0].gate.resolve(view(A, { summary: "甲文全文" }));
    await harness.flush();
    expect(readerTitle(host)).toBe("乙文");
    expect(host.querySelector(".reader-summary")).toBeNull();
    expect(host.querySelector(".drawer.reader.has-translation")).toBeNull();
    expect(host.querySelector(".toast")?.textContent ?? "").not.toContain("全文已更新");
  });

  it("late failure of a superseded op neither toasts nor clears the retry's busy state", async () => {
    const summaries = gated<string>("summarize");
    const { host } = await mountHome();
    await openResolved(host, A);

    click(readerAction(host, "生成本篇摘要"));
    await harness.flush();
    click(navButton(host, "下一篇"));
    await harness.flush();
    readers.at(-1)!.gate.resolve(view(B));
    await harness.flush();
    click(navButton(host, "上一篇"));
    await harness.flush();
    readers.at(-1)!.gate.resolve(view(A));
    await harness.flush();
    expect(readerTitle(host)).toBe("甲文");
    expect(progress(host)).toBe("");

    click(readerAction(host, "生成本篇摘要"));
    await harness.flush();
    expect(summaries.map((c) => c.id)).toEqual([A, A]);
    expect(progress(host)).toContain("摘要");

    summaries[0].gate.reject(new Error("upstream 500"));
    await harness.flush();
    expect(progress(host)).toContain("摘要");
    expect(host.querySelector(".toast")).toBeNull();

    summaries[1].gate.resolve("重试成功");
    await harness.flush();
    expect(progress(host)).toBe("");
    expect(host.querySelector(".reader-summary")?.textContent).toContain("重试成功");
  });

  it("late media prefetch and media:ready for A do not leak into B", async () => {
    const prefetches = gated<ReaderView["media"]>("prefetch");
    const { host } = await mountHome();
    const img = { t: "img", src: "https://cdn.example/a.webp", alt: "甲图" } as const;
    await openResolved(host, A, {
      document: { v: 1, blocks: [img] },
      media: [{ src: img.src, href: "", width: null, height: null, error: "" }],
    });
    await harness.flush();
    expect(prefetches.map((c) => c.id)).toEqual([A]);
    expect(host.textContent).toContain("配图加载中");

    click(navButton(host, "下一篇"));
    await harness.flush();
    readers.at(-1)!.gate.resolve(
      view(B, {
        document: { v: 1, blocks: [{ t: "img", src: "https://cdn.example/b.webp", alt: "乙图" }] },
        media: [{ src: "https://cdn.example/b.webp", href: "rrss://media/2", width: null, height: null, error: "" }],
      }),
    );
    await harness.flush();
    expect(readerTitle(host)).toBe("乙文");
    expect(host.textContent).not.toContain("配图加载中");
    expect(prefetches).toHaveLength(1);

    prefetches[0].gate.resolve([{ src: img.src, href: "rrss://media/1", width: null, height: null, error: "" }]);
    act(() => {
      box.events!.emit("media:ready", { article_id: A, media: { src: img.src, href: "rrss://media/1", width: null, height: null, error: "" } });
    });
    await harness.flush();
    expect(readerTitle(host)).toBe("乙文");
    expect(host.querySelector('img[src="rrss://media/1"]')).toBeNull();
    expect(host.textContent).not.toContain("配图加载中");
  });
});
