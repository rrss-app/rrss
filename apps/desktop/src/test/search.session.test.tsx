import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import type { SearchHitView, SearchQuery } from "../lib/api";
import { createApiStub, type ApiStub } from "./api";
import { deferred } from "./deferred";
import { createEventStub, type EventStub } from "./events";
import { ARTICLE_A, ARTICLE_B } from "./fixtures/reader";
import { click, handleShellReads, pressKey, typeInto } from "./fixtures/shell";
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

function hit(id: number, title: string): SearchHitView {
  return { id, title, feed_title: "示例信源", feed_icon_href: "", published_at: null, score: 1, snippet: "", read: false, starred: false, title_match: true, body_match: false };
}

describe("search session", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };
  let clock: ReturnType<typeof harness.useClock>;

  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    box.stub = createApiStub();
    box.events = createEventStub();
    handleShellReads(box.stub);
    clock = harness.useClock();
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
  });

  async function openPalette() {
    const { host } = harness.mount(<App />);
    await harness.flush();
    click(host.querySelector(".sidebar-search"));
    await harness.flush();
    const input = host.querySelector<HTMLInputElement>(".command-palette-input");
    expect(input).not.toBeNull();
    return { host, input: input! };
  }

  const titles = (host: HTMLElement) =>
    [...host.querySelectorAll(".command-palette-title")].map((el) => el.textContent);

  it("keeps the debounce timer while another thread streams", async () => {
    const queries: string[] = [];
    box.stub!.handle("searchArticles", (q: SearchQuery) => {
      queries.push(q.query);
      expect(q).toEqual({ query: "乙文", scope: "all", feed_ids: [], published_since: null, limit: 50 });
      return [hit(ARTICLE_B, "乙文")];
    });
    const { host, input } = await openPalette();
    typeInto(input, "乙文");
    // 后台线程每 60ms 推一次流式快照，根壳随之重渲染；防抖计时不能因此重置。
    for (let i = 0; i < 6; i++) {
      await harness.flush();
      act(() => box.events!.emit("chat:delta", { thread_id: 7, text: `chunk ${i}`, document: null }));
      await harness.flush();
      await clock.advance(60);
    }
    expect(queries).toEqual(["乙文"]);
    expect(titles(host)).toEqual(["乙文"]);
  });

  it("shows a host failure with retry instead of sample results", async () => {
    let fail = true;
    box.stub!.handle("searchArticles", () => {
      if (fail) throw new Error("index locked");
      return [hit(ARTICLE_A, "甲文")];
    });
    const { host, input } = await openPalette();
    typeInto(input, "甲文");
    await clock.advance(200);
    await harness.flush();
    expect(titles(host)).toEqual([]);
    const alert = host.querySelector(".command-palette [role=alert]");
    expect(alert?.textContent).toContain("搜索失败");
    expect(alert?.textContent).toContain("index locked");

    fail = false;
    click([...host.querySelectorAll(".command-palette button")].find((b) => b.textContent === "重试"));
    await harness.flush();
    expect(box.stub!.callCount("searchArticles")).toBe(2);
    expect(titles(host)).toEqual(["甲文"]);
    expect(host.querySelector(".command-palette [role=alert]")).toBeNull();
  });

  it("never lets a stale query replace the newer word's results, even after close", async () => {
    const gates = new Map<string, ReturnType<typeof deferred<SearchHitView[]>>>();
    box.stub!.handle("searchArticles", (q: SearchQuery) => {
      const gate = deferred<SearchHitView[]>();
      gates.set(q.query, gate);
      return gate.promise;
    });
    const { host, input } = await openPalette();
    typeInto(input, "甲文");
    await clock.advance(200);
    typeInto(input, "乙文");
    await clock.advance(200);
    expect([...gates.keys()]).toEqual(["甲文", "乙文"]);
    expect(host.querySelector(".command-palette [role=status]")?.textContent).toContain("搜索中");

    gates.get("乙文")!.resolve([hit(ARTICLE_B, "乙文")]);
    await harness.flush();
    expect(titles(host)).toEqual(["乙文"]);
    gates.get("甲文")!.resolve([hit(ARTICLE_A, "甲文")]);
    await harness.flush();
    expect(titles(host)).toEqual(["乙文"]);

    // 关闭后再打开：旧词、旧结果与迟到结果都不复现。
    typeInto(input, "丙文");
    await clock.advance(200);
    pressKey("Escape", {}, input);
    await harness.flush();
    expect(host.querySelector(".command-palette")).toBeNull();
    click(host.querySelector(".sidebar-search"));
    await harness.flush();
    gates.get("丙文")!.resolve([hit(3, "丙文")]);
    await harness.flush();
    expect(host.querySelector<HTMLInputElement>(".command-palette-input")?.value).toBe("");
    expect(titles(host)).toEqual([]);
    expect(host.querySelector(".command-palette [role=listbox]")).toBeNull();
  });
});
