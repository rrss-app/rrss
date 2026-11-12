import { handleReaderReads } from "./fixtures/shell";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { createApiStub, type ApiStub } from "./api";
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

/** 宿主返回的坏 IR：缺 inlines、未知块 / 行内类型、非对象块、旧版本号。单篇坏文档不能让整个窗口白屏。 */
const BROKEN_DOCUMENT = {
  v: 1,
  blocks: [
    { t: "p" },
    { t: "p", inlines: [{ t: "t", text: "幸存段落" }, { t: "zzz", text: "未知行内" }, null, { t: "a", children: "x" }] },
    { t: "wtf", inlines: [] },
    "not a block",
    { t: "h", level: "big", inlines: [{ t: "t", text: "标题" }] },
    { t: "li", indent: 1, marker: "•", inlines: [{ t: "s", children: [{ t: "t", text: "加粗项" }] }] },
    { t: "img", src: 12 },
    { t: "pre", text: "code" },
    { t: "hr" },
  ],
};

describe("reader recovery from bad documents", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };
  let errorSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    box.stub = createApiStub();
    handleReaderReads(box.stub);
    box.events = createEventStub();
    box.stub.handle("focus", () => TWO_ARTICLE_PAGE);
    box.stub.handle("feeds", () => ({ folders: [], uncategorized: [], folder_combo: [] }));
    box.stub.handle("threadList", () => ({ folders: [], threads: [], archived_count: 0 }));
    harness.useClock();
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1024);
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
  });

  function openFromList(host: HTMLElement, title: string) {
    const card = [...host.querySelectorAll("article")].find((a) => a.textContent?.includes(title));
    expect(card).toBeTruthy();
    act(() => {
      card!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
  }

  function backButton(host: HTMLElement) {
    return [...host.querySelectorAll<HTMLButtonElement>(".reader-head-nav button")].find((b) => b.textContent === "返回");
  }

  it("renders the valid parts of a malformed document and still returns to the list", async () => {
    box.stub!.handle("openArticle", (id: number) => readerView(id, id === ARTICLE_A ? { document: BROKEN_DOCUMENT } : {}));
    const { host } = await (async () => {
      const mounted = harness.mount(<App />);
      await harness.flush();
      return mounted;
    })();
    expect(host.textContent).toContain("甲文");

    openFromList(host, "甲文");
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
    const body = host.querySelector("[data-reader-document]");
    expect(body).not.toBeNull();
    expect(body!.textContent).toContain("幸存段落");
    expect(body!.textContent).toContain("加粗项");
    expect(body!.textContent).toContain("code");
    expect(body!.textContent).not.toContain("未知行内");
    expect(body!.querySelector("h1")).not.toBeNull();
    expect(body!.querySelector("hr")).not.toBeNull();
    expect(errorSpy).not.toHaveBeenCalled();

    act(() => {
      backButton(host)!.click();
    });
    await harness.flush();
    expect(host.querySelector(".reader-stage")).toBeNull();
    expect(host.textContent).toContain("乙文");

    // 下一篇正常文档不受上一篇影响。
    openFromList(host, "乙文");
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("乙文");
    expect(host.querySelector("[data-reader-document]")?.textContent).toContain(`正文 ${ARTICLE_B}`);
  });

  it("degrades a document with an unknown shape to an empty body instead of crashing", async () => {
    box.stub!.handle("openArticle", (id: number) => readerView(id, { document: { v: 2, blocks: "nope" } }));
    const mounted = harness.mount(<App />);
    await harness.flush();
    openFromList(mounted.host, "甲文");
    await harness.flush();
    expect(mounted.host.querySelector(".reader-title")?.textContent).toBe("甲文");
    expect(mounted.host.querySelector("[data-reader-document]")).not.toBeNull();
    expect(errorSpy).not.toHaveBeenCalled();
  });
});
