import { act, StrictMode, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../lib/api";
import { trackReadSession } from "../../lib/signals";
import { readerView } from "../../test/fixtures/reader";
import { createReactHarness } from "../../test/react";
import { createReaderRequestTracker } from "./readerRequests";
import { inferReadingVia, useReadingSession } from "./useReadingSession";
import type { ReaderOpenOptions } from "./ReaderHost";

vi.mock("../../lib/api", () => ({ api: { readingSpeeds: vi.fn().mockResolvedValue(null) } }));
vi.mock("../../lib/signals", () => ({ trackReadSession: vi.fn() }));
vi.mock("./readerViewport", () => ({
  observeReaderViewport: (_body: HTMLElement, changed: (value: { visible: number[] }) => void) => {
    changed({ visible: [0] });
    return { read: () => changed({ visible: [0] }), dispose: vi.fn() };
  },
}));

describe("reading entry inference", () => {
  it.each<[ReaderOpenOptions | undefined, ReturnType<typeof inferReadingVia>]>([
    [{ returnToList: true }, "focus"], [{ via: "drawer", returnToList: true }, "drawer"],
    [{ skipNav: true }, "digest"], [undefined, "drawer"],
    [{ citationSource: { kind: "new" }, returnToList: true }, "citation"],
    [{ citationSource: { kind: "digest", digestId: 3 }, skipNav: true }, "citation"],
    [{ citationSource: { kind: "thread", threadId: 2, title: "discussion" } }, "citation"],
    [{ anchor: { blockId: 0 }, skipNav: true }, "citation"],
    [{ search: { query: "word", results: [1], bodyMatch: true }, citationSource: { kind: "new" } }, "search"],
  ])("infers %j as %s", (options, expected) => expect(inferReadingVia(options)).toBe(expected));
});

describe("reading session lifecycle and activity", () => {
  const harness = createReactHarness();
  const tracker = createReaderRequestTracker();
  let focused: boolean;
  let visibility: DocumentVisibilityState;
  beforeEach(() => {
    focused = true; visibility = "visible";
    vi.mocked(api.readingSpeeds).mockResolvedValue({ chinese_original: null, chinese_translated: null, english_original: null, english_translated: null });
    vi.mocked(trackReadSession).mockClear();
    vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
    vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility);
  });
  afterEach(async () => {
    harness.cleanup(); await harness.flush();
    vi.restoreAllMocks();
  });

  function setup(confirmed = true, strict = false) {
    const clock = harness.useClock();
    let options: Parameters<typeof useReadingSession>[0] = {
      open: true, dismissible: true, inert: false, via: "focus", translated: false,
      view: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "中".repeat(4_000) }] }] } }),
      ticket: tracker.open(1), openedAt: confirmed ? Date.now() : null,
    };
    let rerender = () => {};
    let controls!: ReturnType<typeof useReadingSession>;
    function Probe() {
      const [, tick] = useState(0);
      rerender = () => tick(n => n + 1);
      controls = useReadingSession(options);
      return <div ref={controls.onBody}><audio /></div>;
    }
    const mounted = harness.mount(strict ? <StrictMode><Probe /></StrictMode> : <Probe />);
    const update = (next: Partial<typeof options>) => act(() => { options = { ...options, ...next }; rerender(); });
    return { clock, update, mounted, finish: () => controls.finish("back") };
  }
  const records = () => vi.mocked(trackReadSession).mock.calls.map(([record]) => record);
  function focus(on: boolean) {
    focused = on;
    act(() => window.dispatchEvent(new Event(on ? "focus" : "blur")));
  }

  it("caches personal speeds, selects language and translation context, and falls back on null or failure", async () => {
    for (const result of [
      { chinese_original: 0.01, chinese_translated: 0.005, english_original: 0.02, english_translated: 0.04 },
      { chinese_original: null, chinese_translated: null, english_original: null, english_translated: null },
      null,
    ]) {
      // 新模块代表新的应用生命周期，不新增生产缓存重置入口。
      vi.resetModules();
      const { useReadingSession: useSession } = await import("./useReadingSession");
      if (result) vi.mocked(api.readingSpeeds).mockResolvedValueOnce(result);
      else vi.mocked(api.readingSpeeds).mockRejectedValueOnce(new Error("unavailable"));
      const requestCount = vi.mocked(api.readingSpeeds).mock.calls.length;
      const models = await import("./readingSession");
      const create = vi.spyOn(models, "createReadingSession");
      let controls!: ReturnType<typeof useSession>;
      let options: Parameters<typeof useSession>[0] = {
        open: false, dismissible: true, view: null, ticket: null, openedAt: null, via: "focus", translated: false,
      };
      let rerender = () => {};
      function Probe() {
        const [, tick] = useState(0);
        rerender = () => tick(n => n + 1);
        controls = useSession(options);
        return <div ref={controls.onBody} />;
      }
      const mounted = harness.mount(<Probe />);
      await harness.flush();
      for (const [chinese, translated] of [
        [true, false], [true, true], [false, false], [false, true],
      ] as const) {
        act(() => {
          options = { ...options, open: true, translated, openedAt: Date.now(), ticket: tracker.open(1),
            view: readerView(1, { title: chinese ? "中文" : "English", document: { v: 1, blocks: [
              { t: "p", inlines: [{ t: "t", text: chinese ? "中".repeat(4_000) : "a".repeat(4_000) }] },
            ] } }) };
          rerender();
        });
        const mode = translated ? "translated" : "original";
        expect(create.mock.calls.at(-1)?.[0].charsPerMs).toEqual(result ? {
          chinese: result[`chinese_${mode}`] ?? undefined,
          english: result[`english_${mode}`] ?? undefined,
        } : undefined);
        controls.finish("back");
      }
      expect(api.readingSpeeds).toHaveBeenCalledTimes(requestCount + 1);
      mounted.unmount(); await harness.flush(); create.mockRestore();
    }
  });

  it("waits for successful opening, writes once on back, and has no parallel session for the same article", async () => {
    const s = setup(false);
    await s.clock.advance(2_000);
    s.update({ openedAt: Date.now() });
    await s.clock.advance(1_000);
    s.update({ ticket: tracker.open(1), openedAt: Date.now() });
    await s.clock.advance(1_000); s.finish(); s.update({ open: false });
    expect(records()).toHaveLength(1);
    expect(records()[0]).toMatchObject({ article_id: 1, active_ms: 2_000, ended_by: "back", reached_end: true });
  });

  it("excludes previews and unconfirmed content", async () => {
    const s = setup(false);
    s.update({ view: readerView(1, { source_hash: "preview-1" }), openedAt: Date.now() });
    await s.clock.advance(1_000); s.update({ open: false });
    expect(records()).toHaveLength(0);
  });

  it("finishes the previous article on next and the current one on pagehide", async () => {
    const s = setup(); await s.clock.advance(1_000);
    s.update({ view: readerView(2), ticket: tracker.open(2), openedAt: Date.now(), via: "drawer" });
    await s.clock.advance(1_000);
    act(() => window.dispatchEvent(new Event("pagehide")));
    s.update({ translated: true }); s.finish();
    expect(records()).toHaveLength(2);
    expect(records()[0]).toMatchObject({ article_id: 1, ended_by: "next" });
    expect(records()[1]).toMatchObject({ article_id: 2, via: "drawer", ended_by: "close" });
  });

  it("pauses under overlays and inert, without triggering the focus timeout", async () => {
    const s = setup(); await s.clock.advance(1_000);
    s.update({ dismissible: false }); await s.clock.advance(601_000);
    s.update({ dismissible: true, inert: true }); await s.clock.advance(1_000);
    s.update({ inert: false }); await s.clock.advance(1_000); s.finish();
    expect(records()[0]).toMatchObject({ active_ms: 2_000, idle_ms: 602_000, ended_by: "back" });
  });

  it("resumes before the timeout", async () => {
    const s = setup(); await s.clock.advance(1_000); focus(false);
    await s.clock.advance(60_000); focus(true);
    await s.clock.advance(1_000); s.finish();
    expect(records()[0]).toMatchObject({ active_ms: 2_000, idle_ms: 60_000 });
  });

  it.each(["blur", "hidden"])("ends after ten minutes of %s, without restarting until another open", async kind => {
    const s = setup(); await s.clock.advance(1_000);
    if (kind === "blur") focus(false);
    else { visibility = "hidden"; act(() => document.dispatchEvent(new Event("visibilitychange"))); }
    await s.clock.advance(600_000);
    expect(records()[0]).toMatchObject({ active_ms: 1_000, idle_ms: 600_000, ended_by: "focus_timeout" });
    visibility = "visible"; focus(true); s.update({ translated: true });
    await s.clock.advance(1_000); s.finish();
    expect(records()).toHaveLength(1);
    s.update({ ticket: tracker.open(1), openedAt: Date.now() });
    await s.clock.advance(1_000); s.finish();
    expect(records()).toHaveLength(2);
  });

  it("treats playing media as ongoing activity and stops refreshing after pause", async () => {
    const s = setup();
    const audio = s.mounted.host.querySelector("audio")!;
    let paused = false;
    vi.spyOn(audio, "paused", "get").mockImplementation(() => paused);
    act(() => audio.dispatchEvent(new Event("playing")));
    await s.clock.advance(200_000);
    paused = true; act(() => audio.dispatchEvent(new Event("pause")));
    await s.clock.advance(120_000); s.finish();
    expect(records()[0]).toMatchObject({ active_ms: 290_000, idle_ms: 30_000 });
  });

  it("refreshes on scroll, wheel, keyboard, selection and throttled pointer movement", async () => {
    const s = setup();
    const body = s.mounted.host.firstElementChild!;
    for (const [target, event] of [[body, "scroll"], [body, "wheel"], [window, "keydown"], [document, "selectionchange"], [window, "pointermove"]] as const) {
      await s.clock.advance(80_000); act(() => target.dispatchEvent(new Event(event)));
    }
    s.finish();
    expect(records()[0]).toMatchObject({ active_ms: 400_000, idle_ms: 0 });
  });

  it("does not create a close record for StrictMode's rehearsal mount", async () => {
    const s = setup(true, true); await harness.flush();
    expect(records()).toHaveLength(0);
    await s.clock.advance(1_000); s.finish();
    expect(records()).toHaveLength(1);
    expect(records()[0].active_ms).toBe(1_000);
  });
});
