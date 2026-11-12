import { act, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import type { ReaderView } from "../lib/api";
import { ReaderOverlay } from "./ReaderOverlay";
import { readerView } from "../test/fixtures/reader";
import { createReactHarness } from "../test/react";
import { click } from "../test/fixtures/shell";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });
const props = {
  dismissible: true, summarizing: false, translating: false, translateMode: "original" as const, mediaLoading: false,
  onClose: () => {}, onCiteArticle: () => {}, onCiteSelection: () => {}, onSummarize: () => {}, onTranslate: () => {},
  onTranslateMode: () => {}, onRate: () => {},
};
const reader = readerView(1, { truncated: true });
function setup(summarizing = false) {
  const cb = { onCiteArticle: vi.fn(), onRegenerate: vi.fn(), onSummarize: vi.fn(), onFetchFulltext: vi.fn(), onStar: vi.fn() };
  let render: (next: ReaderView) => void = () => {};
  function Screen() {
    const [article, setArticle] = useState(reader);
    render = (next) => { act(() => setArticle(next)); };
    return <ReaderOverlay {...props} reader={article} summarizing={summarizing} {...cb} />;
  }
  const { host } = harness.mount(<Screen />);
  const tools = host.querySelector<HTMLDetailsElement>(".reader-tools")!;
  const trigger = tools.querySelector("summary")!;
  const button = (name: string) => [...host.querySelectorAll("button")].find((item) => (item.getAttribute("aria-label") ?? item.textContent) === name)!;
  return { host, cb, render, tools, trigger, button };
}
it("keeps secondary actions collapsed and closes the panel after an action", () => {
  const { tools, trigger, button, cb } = setup();
  expect(tools.open).toBe(false);
  for (const [label, callback] of [["引用全文到对话", cb.onCiteArticle], ["重新翻译全文", cb.onRegenerate], ["抓取全文", cb.onFetchFulltext]] as const) {
    act(() => { tools.open = true; button(label).click(); });
    expect(callback).toHaveBeenCalledOnce();
    expect(tools.open).toBe(false);
    expect(document.activeElement).toBe(trigger);
  }
});
it("Escape closes the tools before the reader", () => {
  const { tools, trigger } = setup();
  const windowKey = vi.fn();
  window.addEventListener("keydown", windowKey);
  try {
    tools.open = true;
    act(() => trigger.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
    expect(tools.open).toBe(false);
    expect(windowKey).not.toHaveBeenCalled();
  } finally { window.removeEventListener("keydown", windowKey); }
});
it("closes on outside pointer interaction and article navigation", () => {
  const { tools, host, render } = setup();
  tools.open = true;
  act(() => host.querySelector(".reader-title")!.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  expect(tools.open).toBe(false);
  tools.open = true;
  render({ ...reader, id: 2 });
  expect(tools.open).toBe(false);
});
it("keeps progress visible while tools are collapsed", () => {
  const { host, tools, button } = setup(true);
  expect(tools.open).toBe(false);
  expect(host.querySelector('[role="status"]')?.textContent).toContain("正在生成摘要");
  expect(button("摘要生成中…").disabled).toBe(true);
});
it("reserves room for the persistent translation controls", () => {
  const { host, render } = setup();
  render({ ...reader, translation_ready: true, translated_document: reader.document });
  expect(host.querySelector(".reader.has-translation")).not.toBeNull();
  expect(host.querySelectorAll('.reader-view-rail [role="radio"]')).toHaveLength(3);
});

it("shows a translation failure without hiding the reader, and allows dismissing it", () => {
  const onDismissNote = vi.fn();
  const { host } = harness.mount(<ReaderOverlay {...props} reader={readerView(1, { translation_note: "翻译失败：HTTP 403 Invalid API Key" })} onDismissNote={onDismissNote} />);
  expect(host.querySelector(".reader-notice")?.textContent).toContain("翻译失败：HTTP 403 Invalid API Key");
  expect(host.querySelector(".reader-stage")).not.toBeNull();
  expect(host.querySelector(".reader-title")?.textContent).toBe("甲文");
  click(host.querySelector(".reader-notice-dismiss"));
  expect(onDismissNote).toHaveBeenCalledOnce();
});
it("shows starred state on the star control", () => {
  const { host } = harness.mount(<ReaderOverlay {...props} reader={readerView(1, { starred: true })} onStar={() => {}} />);
  expect(host.querySelector(".reader-head-actions button[aria-label='取消收藏']")?.getAttribute("aria-pressed")).toBe("true");
});
it("connects all four ending actions to the existing reader callbacks and reflects saved feedback", () => {
  const onRate = vi.fn(), onStar = vi.fn(), onCiteArticle = vi.fn();
  let update = (_next: ReaderView) => {};
  function Screen() {
    const [value, setValue] = useState(reader);
    update = setValue;
    return <ReaderOverlay {...props} reader={value} onRate={onRate} onStar={onStar} onCiteArticle={onCiteArticle} />;
  }
  const { host } = harness.mount(<Screen />);
  const actions = () => [...host.querySelectorAll<HTMLButtonElement>(".reader-ending-actions button")];
  for (const button of actions()) click(button);
  expect(onRate.mock.calls).toEqual([[true], [false]]);
  expect(onStar).toHaveBeenCalledOnce();
  expect(onCiteArticle).toHaveBeenCalledOnce();
  act(() => update({ ...reader, feedback: -1, starred: true }));
  expect(actions().map(button => button.getAttribute("aria-pressed"))).toEqual(["false", "true", "true", null]);
});
it("flags a summary written from an older body and offers to regenerate it", () => {
  const onSummarize = vi.fn();
  const fresh = harness.mount(<ReaderOverlay {...props} reader={readerView(4, { summary: "旧摘要。" })} />);
  expect(fresh.host.querySelector(".reader-summary-stale")).toBeNull();
  harness.cleanup();
  const { host } = harness.mount(<ReaderOverlay {...props} reader={readerView(4, { summary: "旧摘要。", summary_stale: true })} onSummarize={onSummarize} />);
  const note = host.querySelector(".reader-summary-stale")!;
  expect(note.textContent).toContain("按之前的正文写的");
  act(() => note.querySelector("button")!.click());
  expect(onSummarize).toHaveBeenCalledOnce();
});
it("explains pruned articles and offers the original and a refetch", () => {
  const onFetchFulltext = vi.fn();
  const { host } = harness.mount(<ReaderOverlay {...props} reader={readerView(3, { pruned: true, truncated: false })} onFetchFulltext={onFetchFulltext} />);
  const notice = host.querySelector(".reader-pruned")!;
  expect(notice.textContent).toContain("正文已按保留策略清理");
  const refetch = [...notice.querySelectorAll("button")].find((b) => b.textContent === "重新抓取正文")!;
  act(() => refetch.click());
  expect(onFetchFulltext).toHaveBeenCalledOnce();
  expect([...host.querySelectorAll(".reader-tools button")].some((b) => b.textContent?.startsWith("抓取全文"))).toBe(true);
});

function measuredReader() {
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const body = this.closest<HTMLElement>(".drawer-body");
    const top = this.dataset.blockId === undefined ? 0 : 400 + Number(this.dataset.blockId) * 200 - (body?.scrollTop ?? 0);
    return { top, bottom: top + 200, left: 0, right: 640, width: 640, height: 200, x: 0, y: top, toJSON: () => ({}) };
  });
  return readerView(1, { document: { v: 1, blocks: Array.from({ length: 12 }, (_, i) => ({ t: "p", inlines: [{ t: "t", text: `段落 ${i}` }] })) } });
}
it("locates a message anchor above the viewport center, highlights briefly and cleans up on close", async () => {
  const clock = harness.useClock();
  const view = measuredReader();
  const { host, unmount } = harness.mount(<ReaderOverlay {...props} reader={view} anchor={{ blockId: 8 }} readingPosition={{ block: "5", offset: 120 }} />);
  const target = host.querySelector<HTMLElement>('[data-reader-document] > [data-block-id="8"]')!;
  expect(target.getBoundingClientRect().top).toBe(120);
  expect(target.classList.contains("reader-block-highlight")).toBe(true);
  await clock.advance(1199);
  expect(target.classList.contains("reader-block-highlight")).toBe(true);
  await clock.advance(1);
  expect(target.classList.contains("reader-block-highlight")).toBe(false);
  unmount();
});
it("locates block 0 (the first body paragraph) and keeps a search hit highlighted", async () => {
  const clock = harness.useClock();
  const view = measuredReader();
  const { host } = harness.mount(<ReaderOverlay {...props} reader={view} anchor={{ blockId: 0, persist: true }} readingPosition={{ block: "5", offset: 120 }} />);
  const target = host.querySelector<HTMLElement>('[data-reader-document] > [data-block-id="0"]')!;
  expect(target.getBoundingClientRect().top).toBe(120);
  await clock.advance(5000);
  expect(target.classList.contains("reader-search-hit")).toBe(true);
});
it.each([undefined, -1, 999])("falls back to the saved reading position for absent or invalid block %s", (blockId) => {
  const view = measuredReader();
  const { host } = harness.mount(<ReaderOverlay {...props} reader={view} anchor={blockId === undefined ? undefined : { blockId }} readingPosition={{ block: "5", offset: 120 }} />);
  expect(host.querySelector<HTMLElement>('[data-reader-document] > [data-block-id="5"]')!.getBoundingClientRect().top).toBe(120);
  expect(host.querySelector(".reader-block-highlight")).toBeNull();
});
