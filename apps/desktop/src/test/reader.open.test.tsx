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

type Pending<T> = { id: number; gate: Deferred<T> };

function gated<T>(name: "openArticle" | "reader" | "summarize" | "translate" | "fetchFulltext") {
  const calls: Pending<T>[] = [];
  box.stub!.handle(name, ((id: number) => {
    const gate = deferred<T>();
    calls.push({ id, gate });
    return gate.promise;
  }) as never);
  return calls;
}

describe("reader open action vs pure snapshot", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };

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

  function openFromList(host: HTMLElement, title: string) {
    const card = [...host.querySelectorAll("article")].find((a) => a.textContent?.includes(title));
    expect(card).toBeTruthy();
    act(() => {
      card!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
  }

  function cardByTitle(host: HTMLElement, title: string) {
    return [...host.querySelectorAll("article")].find((a) => a.textContent?.includes(title));
  }

  function readerAction(host: HTMLElement, label: string) {
    return [...host.querySelectorAll<HTMLButtonElement>(".reader-head button")].find((b) =>
      (b.getAttribute("aria-label") ?? b.textContent)?.includes(label),
    );
  }

  it("user open uses openArticle, not the pure reader snapshot", async () => {
    const opens = gated<ReaderView>("openArticle");
    box.stub!.handle("reader", () => {
      throw new Error("pure reader must not run on user open");
    });
    const { host } = await mountHome();
    expect(cardByTitle(host, "甲文")?.className).toContain("unread");

    openFromList(host, "甲文");
    await harness.flush();
    expect(opens.map((c) => c.id)).toEqual([ARTICLE_A]);
    expect(box.stub!.callCount("reader")).toBe(0);
    expect(cardByTitle(host, "甲文")?.className).toContain("unread");

    opens[0].gate.resolve(readerView(ARTICLE_A));
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
    expect(cardByTitle(host, "甲文")?.className).not.toContain("unread");
    expect(box.stub!.callCount("reader")).toBe(0);
  });

  it("failed open does not sync read, even when a later open succeeds", async () => {
    const opens = gated<ReaderView>("openArticle");
    const { host } = await mountHome();

    openFromList(host, "甲文");
    await harness.flush();
    openFromList(host, "乙文");
    await harness.flush();
    expect(opens.map((c) => c.id)).toEqual([ARTICLE_A, ARTICLE_B]);

    opens[0].gate.reject(new Error("missing article"));
    await harness.flush();
    expect(cardByTitle(host, "甲文")?.className).toContain("unread");
    expect(cardByTitle(host, "乙文")?.className).toContain("unread");
    expect(host.querySelector(".reader-stage")).toBeNull();

    opens[1].gate.resolve(readerView(ARTICLE_B));
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("乙文");
    expect(cardByTitle(host, "甲文")?.className).toContain("unread");
    expect(cardByTitle(host, "乙文")?.className).not.toContain("unread");
  });

  it("stale successful open still syncs the original article id", async () => {
    const opens = gated<ReaderView>("openArticle");
    const { host } = await mountHome();
    openFromList(host, "甲文");
    await harness.flush();
    openFromList(host, "乙文");
    await harness.flush();

    opens[1].gate.resolve(readerView(ARTICLE_B));
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("乙文");
    expect(cardByTitle(host, "乙文")?.className).not.toContain("unread");
    expect(cardByTitle(host, "甲文")?.className).toContain("unread");

    opens[0].gate.resolve(readerView(ARTICLE_A, { summary: "迟到的甲文" }));
    await harness.flush();
    expect(host.querySelector(".reader-title")?.textContent).toBe("乙文");
    expect(host.querySelector(".reader-summary")?.textContent ?? "").not.toContain("迟到的甲文");
    expect(cardByTitle(host, "甲文")?.className).not.toContain("unread");
  });

  it("summary / translate / fulltext refresh stay on their own commands, not openArticle", async () => {
    const opens = gated<ReaderView>("openArticle");
    const summaries = gated<string>("summarize");
    const translations = gated<TranslationReply>("translate");
    const fulltexts = gated<ReaderView>("fetchFulltext");
    const { host } = await mountHome();
    openFromList(host, "甲文");
    await harness.flush();
    opens[0].gate.resolve(readerView(ARTICLE_A, { truncated: true }));
    await harness.flush();

    act(() => {
      readerAction(host, "生成本篇摘要")!.click();
      readerAction(host, "翻译")!.click();
      readerAction(host, "抓取全文")!.click();
    });
    await harness.flush();
    expect(opens).toHaveLength(1);
    expect(box.stub!.callCount("reader")).toBe(0);
    expect(summaries.map((c) => c.id)).toEqual([ARTICLE_A]);
    expect(translations.map((c) => c.id)).toEqual([ARTICLE_A]);
    expect(fulltexts.map((c) => c.id)).toEqual([ARTICLE_A]);

    summaries[0].gate.resolve("摘要正文");
    translations[0].gate.resolve(translationReply(readerView(ARTICLE_A), translationSnapshot(readerView(ARTICLE_A), { phase: "completed", save: "saved", units: ["done", "done"], title: "Article A", blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Translated" }] } }] })));
    fulltexts[0].gate.resolve(readerView(ARTICLE_A, { summary: "全文" }));
    await harness.flush();
    expect(opens).toHaveLength(1);
    expect(box.stub!.callCount("openArticle")).toBe(1);
    expect(box.stub!.callCount("reader")).toBe(0);
  });
});
