import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { ReaderView } from "../lib/api";
import { ReaderOverlay } from "./ReaderOverlay";

vi.mock("./BlockView", async () => {
  const actual = await vi.importActual<typeof import("./BlockView")>("./BlockView");
  return {
    ...actual,
    BlockView(props: Parameters<typeof actual.BlockView>[0]) {
      if ((props.document as { boom?: boolean } | null)?.boom) throw new Error("渲染崩了");
      return actual.BlockView(props);
    },
  };
});

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

const base: ReaderView = {
  source_hash: "reader-recovery-source",
  id: 1, title: "坏文章", meta: "测试源", feed_icon_href: "", url: "https://example.test/article", summary: "",
  score: 0, feedback: 0, starred: false, reasons: [], media: [],
  document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "正常正文" }] }] },
  translated_document: null, translated_title: "", translation_ready: false, translation_note: "", truncated: false, pruned: false, summary_stale: false, embed_covers: [],
};

let cleanup = () => {};
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

function setup() {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const onClose = vi.fn();
  const onPrev = vi.fn();
  const render = (article: ReaderView) => act(() => root.render(
    <ReaderOverlay reader={article} dismissible summarizing={false} translating={false} translateMode="original" mediaLoading={false}
      onClose={onClose} onCiteArticle={vi.fn()} onCiteSelection={vi.fn()} onSummarize={vi.fn()} onTranslate={vi.fn()}
      onTranslateMode={vi.fn()} onRate={vi.fn()} onPrev={onPrev} onNext={vi.fn()} hasPrev hasNext />,
  ));
  cleanup = () => { act(() => root.unmount()); host.remove(); };
  return { host, render, onClose, onPrev };
}

describe("reader body error boundary", () => {
  it("keeps the header, back and paging usable when the body throws, and recovers on the next article", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { host, render, onClose, onPrev } = setup();
    render({ ...base, document: { boom: true } });
    expect(host.querySelector(".reader-stage")).not.toBeNull();
    expect(host.querySelector(".reader-title")?.textContent).toBe("坏文章");
    expect(host.querySelector(".reader-body-error")?.textContent).toContain("正文显示不出来");
    expect(host.querySelector("[data-reader-document]")).toBeNull();

    const back = [...host.querySelectorAll<HTMLButtonElement>(".reader-head-nav button")].find((b) => b.textContent === "返回")!;
    act(() => back.click());
    expect(onClose).toHaveBeenCalledOnce();
    act(() => host.querySelector<HTMLButtonElement>('.reader-pager button[aria-label="上一篇"]')!.click());
    expect(onPrev).toHaveBeenCalledOnce();

    render({ ...base, id: 2, title: "好文章" });
    expect(host.querySelector(".reader-body-error")).toBeNull();
    expect(host.querySelector("[data-reader-document]")?.textContent).toContain("正常正文");
  });
});
