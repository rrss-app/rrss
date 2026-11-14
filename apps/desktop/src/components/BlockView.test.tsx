import { deferred } from "../test/deferred";
import { act, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { BlockView, LIBRARY_FIXTURE, LIBRARY_FIXTURE_EN, parseDocument, readSelection } from "./BlockView";
import { api } from "../lib/api";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

function mount(node: ReactNode) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(node);
  });
  return { host, root, unmount: () => { act(() => root.unmount()); host.remove(); } };
}

describe("tables", () => {
  it("renders a header row, aligned cells and inline formatting, and links stay clickable", () => {
    const opened: string[] = [];
    const doc = { v: 1, blocks: [
      { t: "p", inlines: [{ t: "t", text: "前一段" }] },
      { t: "table", align: ["", "r"], head: [[{ t: "t", text: "检验项" }], [{ t: "t", text: "数量" }]],
        rows: [[[{ t: "s", children: [{ t: "t", text: "表格" }] }], [{ t: "t", text: "948" }]],
          [[{ t: "a", href: "https://example.test/x", children: [{ t: "t", text: "链接" }] }], [{ t: "c", text: "1169" }]]] },
    ] };
    const { host, unmount } = mount(<BlockView document={doc} onOpenUrl={(href) => opened.push(href)} />);
    const table = host.querySelector('[data-block-id="1"] table')!;
    expect([...table.querySelectorAll("th")].map((th) => th.textContent)).toEqual(["检验项", "数量"]);
    expect([...table.querySelectorAll("tbody tr")].map((tr) => [...tr.children].map((td) => td.textContent))).toEqual([["表格", "948"], ["链接", "1169"]]);
    expect((table.querySelector("tbody td:nth-child(2)") as HTMLElement).style.textAlign).toBe("right");
    act(() => (table.querySelector("a") as HTMLElement).click());
    expect(opened).toEqual(["https://example.test/x"]);
    unmount();
  });
});

describe("Block IR", () => {
  it("parses fixture document", () => {
    const doc = parseDocument(LIBRARY_FIXTURE);
    expect(doc.v).toBe(1);
    expect(doc.blocks.map((b) => b.t)).toEqual([
      "h",
      "p",
      "p",
      "img",
      "li",
      "li",
      "q",
      "pre",
      "p",
      "embed",
    ]);
  });

  it("returns empty document for junk", () => {
    expect(parseDocument(null).blocks).toEqual([]);
    expect(parseDocument("html").blocks).toEqual([]);
  });
});

describe("selection and links", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.replaceChildren();
  });

  it("opens a link via onOpenUrl and does not cite", () => {
    const opened: string[] = [];
    const cited: string[] = [];
    const { host, unmount } = mount(
      <BlockView
        document={LIBRARY_FIXTURE}
        selectable
        onOpenUrl={(href) => opened.push(href)}
        onCite={(excerpt) => cited.push(excerpt)}
      />,
    );
    const link = host.querySelector("a") as HTMLAnchorElement;
    expect(link.textContent).toBe("时刻表");
    act(() => {
      link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    });
    expect(opened).toEqual(["https://library.example/hours"]);
    expect(cited).toEqual([]);
    unmount();
  });

  it("clips cross-block selections and excludes source buttons and list markers", () => {
    const onCiteBlocks = vi.fn();
    const document = { v: 1, blocks: [
      { t: "h", level: 2, inlines: [{ t: "t", text: "简报" }] },
      { t: "li", marker: "•", inlines: [{ t: "t", text: "甲" }, { t: "s", children: [{ t: "t", text: "重点" }] }, { t: "br" }, { t: "t", text: "尾" }] },
      { t: "li", marker: "•", inlines: [{ t: "t", text: "乙条目" }] },
    ] };
    const { host, unmount } = mount(<BlockView document={document} selectable onCiteBlocks={onCiteBlocks}
      sources={[{ block_id: 1, sources: [{ article_id: 1, title: "来源标题", feed_title: "测试源", number: 1, summary: null, published_at: null, status: "available" }] }, { block_id: 2, sources: [] }]}
      onOpenArticle={() => {}} />);
    const first = host.querySelector('[data-block-id="1"]')!;
    const second = host.querySelector('[data-block-id="2"]')!;
    const selection = window.getSelection()!;
    selection.setBaseAndExtent(second, second.childNodes.length, first, 0);
    expect(readSelection()?.blocks).toEqual([{ blockId: 1, excerpt: "甲重点\n尾" }, { blockId: 2, excerpt: "乙条目" }]);
    act(() => first.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, altKey: true })));
    expect(onCiteBlocks).toHaveBeenCalledWith([{ blockId: 1, excerpt: "甲重点\n尾" }, { blockId: 2, excerpt: "乙条目" }]);
    expect(selection.isCollapsed).toBe(true);
    unmount();
  });

  it("matches digest sources by block_id even when source lines are not in list order", () => {
    const opened: number[] = [];
    const document = { v: 1, blocks: [
      { t: "h", level: 3, inlines: [{ t: "t", text: "本期" }] },
      { t: "li", marker: "•", inlines: [{ t: "t", text: "第一条" }] },
      { t: "p", inlines: [{ t: "t", text: "间隔段落" }] },
      { t: "li", marker: "•", inlines: [{ t: "t", text: "第二条" }] },
    ] };
    const source = (article_id: number, number: number) => ({
      article_id, title: `文章 ${article_id}`, feed_title: `来源 ${article_id}`,
      number, summary: null, published_at: null, status: "available",
    });
    const { host, unmount } = mount(<BlockView document={document} sources={[
      { block_id: 3, sources: [source(33, 2)] },
      { block_id: 1, sources: [source(11, 1)] },
    ]} onOpenArticle={(id) => opened.push(id)} />);
    const first = host.querySelector('[data-block-id="1"] .digest-ref') as HTMLButtonElement;
    const second = host.querySelector('[data-block-id="3"] .digest-ref') as HTMLButtonElement;
    expect(first.getAttribute("aria-label")).toBe("来源 1：《文章 11》，来源 11");
    expect(second.textContent).toBe("2");
    expect(first.closest(".digest-refs")?.hasAttribute("data-selection-ignore")).toBe(true);
    act(() => { first.click(); second.click(); });
    expect(opened).toEqual([11, 33]);
    unmount();
  });

  it("closes digest source actions when an invalid top-level block shifts block IDs", () => {
    const onCiteBlocks = vi.fn();
    const opened = vi.fn();
    const badDoc = { v: 1, blocks: [
      { t: "h", level: 3, inlines: [{ t: "t", text: "本期" }] },
      { t: "unsupported", text: "丢弃后会移动块号" },
      { t: "li", marker: "•", inlines: [{ t: "t", text: "第二条" }] },
    ] };
    const { host, unmount } = mount(<BlockView document={badDoc} selectable onCiteBlocks={onCiteBlocks}
      sources={[{ block_id: 2, sources: [{ article_id: 2, title: "文章 2", feed_title: "测试源", number: 1, summary: null, published_at: null, status: "available" }] }]}
      onOpenArticle={opened} />);
    expect(host.textContent).toContain("第二条");
    expect(host.querySelector(".digest-ref")).toBeNull();
    const block = host.querySelector('[data-block-id="1"]')!;
    const range = document.createRange();
    range.selectNodeContents(block);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    act(() => block.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, altKey: true })));
    expect(onCiteBlocks).not.toHaveBeenCalled();
    expect(opened).not.toHaveBeenCalled();
    window.getSelection()?.removeAllRanges();
    unmount();
  });

  it("rejects a selection crossing two document roots", () => {
    const onCite = vi.fn();
    const { host, unmount } = mount(<>
      <BlockView document={LIBRARY_FIXTURE} selectable onCite={onCite} />
      <BlockView document={LIBRARY_FIXTURE_EN} selectable onCite={onCite} />
    </>);
    const roots = host.querySelectorAll<HTMLElement>("[data-reader-document]");
    const range = document.createRange();
    range.setStart(roots[0], 0);
    range.setEnd(roots[1], 1);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    expect(readSelection(roots[1])).toBeNull();
    act(() => roots[1].dispatchEvent(new MouseEvent("mouseup", { bubbles: true })));
    expect(document.querySelector(".sel-menu")).toBeNull();
    expect(onCite).not.toHaveBeenCalled();
    window.getSelection()?.removeAllRanges();
    unmount();
  });

  it("closes a stale selection menu when the document changes", () => {
    const onCite = vi.fn();
    const { host, root, unmount } = mount(<BlockView document={LIBRARY_FIXTURE} selectable onCite={onCite} />);
    const paragraph = host.querySelector("p")!;
    const range = document.createRange();
    range.selectNodeContents(paragraph);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    act(() => paragraph.dispatchEvent(new MouseEvent("mouseup", { bubbles: true })));
    expect(document.querySelector(".sel-menu")).not.toBeNull();
    act(() => root.render(<BlockView document={LIBRARY_FIXTURE_EN} selectable onCite={onCite} />));
    expect(document.querySelector(".sel-menu")).toBeNull();
    expect(onCite).not.toHaveBeenCalled();
    window.getSelection()?.removeAllRanges();
    unmount();
  });

  it("shows copy/cite menu on drag, not an immediate citation", () => {
    const cited: string[] = [];
    const { host, unmount } = mount(
      <BlockView document={LIBRARY_FIXTURE} selectable onCite={(excerpt) => cited.push(excerpt)} />,
    );
    const p = host.querySelectorAll("p")[1] as HTMLParagraphElement;
    const text = p.firstChild as Text;
    const range = document.createRange();
    range.selectNodeContents(p);
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(range);
    expect(readSelection()?.excerpt).toContain("儿童区");
    act(() => {
      p.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 40, clientY: 80 }));
    });
    expect(cited).toEqual([]);
    const menu = document.querySelector(".sel-menu");
    expect(menu).toBeTruthy();
    expect(menu?.textContent).toContain("复制");
    expect(menu?.textContent).toContain("引用");
    act(() => {
      (menu?.querySelector('[role="menuitem"]:last-child') as HTMLButtonElement).click();
    });
    expect(cited[0]).toContain("儿童区");
    expect(document.querySelector(".sel-menu")).toBeNull();
    void text;
    unmount();
  });
});

describe("code blocks", () => {
  it("colors labelled code and citing a selection inside it keeps the original text", async () => {
    const code = "fn main() {\n    let name = \"rrss\"; // greet\n}";
    const doc = { v: 1, blocks: [{ t: "pre" as const, lang: "rust", text: code }] };
    const cited: string[] = [];
    const { host, unmount } = mount(<BlockView document={doc} selectable onCite={(excerpt) => cited.push(excerpt)} />);
    await vi.waitFor(() => expect(host.querySelector("pre code .code-keyword")).toBeTruthy());
    const element = host.querySelector("pre code") as HTMLElement;
    expect(element.textContent).toBe(code);
    const range = document.createRange();
    range.selectNodeContents(element);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    act(() => {
      element.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 40, clientY: 40 }));
    });
    act(() => {
      (document.querySelector('.sel-menu [role="menuitem"]:last-child') as HTMLButtonElement).click();
    });
    expect(cited).toEqual([code]);
    window.getSelection()?.removeAllRanges();
    unmount();
  });
});

describe("images", () => {
  it("renders cached media href and loading placeholder", () => {
    const { host, unmount } = mount(
      <BlockView
        document={{ v: 1, blocks: [{ t: "img", src: "https://cdn.example/a.webp", alt: "封面" }] }}
        media={[{ src: "https://cdn.example/a.webp", href: "rrss://localhost/media/1", width: null, height: null, error: "" }]}
      />,
    );
    const img = host.querySelector("img") as HTMLImageElement;
    expect(img.getAttribute("src")).toBe("rrss://localhost/media/1");
    unmount();

    const loading = mount(
      <BlockView
        document={{ v: 1, blocks: [{ t: "img", src: "https://cdn.example/a.webp", alt: "封面" }] }}
        mediaLoading
      />,
    );
    expect(loading.host.textContent).toContain("配图加载中");
    expect(loading.host.textContent).not.toContain("重试");
    loading.unmount();
  });

  it("shows the cache failure and the image after retry", async () => {
    const src = "https://cdn.example/a.avif";
    const ready = { src, href: "rrss://localhost/media/4", width: 32, height: 16, error: "" };
    const pending = deferred<typeof ready>();
    vi.spyOn(api, "retryImage").mockImplementation(() => pending.promise);
    const opened = vi.spyOn(window, "open").mockImplementation(() => null);
    function Harness() {
      const [media, setMedia] = useState([{ src, href: "", width: null as number | null, height: null as number | null, error: "image_decode_failed" }]);
      return (
        <BlockView
          articleId={3}
          document={{ v: 1, blocks: [{ t: "img", src, alt: "封面" }] }}
          media={media}
          onMediaRef={(ref) => setMedia((cur) => cur.map((item) => item.src === ref.src ? ref : item))}
        />
      );
    }
    const { host, unmount } = mount(<Harness />);
    expect(host.textContent).toContain("图片未能缓存（解码失败）");
    expect(host.textContent).toContain("封面");
    const button = (label: string) => [...host.querySelectorAll("button")].find((item) => item.textContent === label) as HTMLButtonElement;
    await act(async () => { button("在浏览器打开图片").click(); });
    expect(opened).toHaveBeenCalledWith(src, "_blank", "noopener,noreferrer");
    await act(async () => { button("重试").click(); });
    const retry = button("重试");
    expect(retry.getAttribute("aria-busy")).toBe("true");
    expect(retry.textContent).toBe("重试");
    await act(async () => { pending.resolve(ready); await pending.promise; });
    expect(host.querySelector("img")?.getAttribute("src")).toBe(ready.href);
    expect(host.textContent).not.toContain("图片未能缓存");
    unmount();
    opened.mockRestore();
    vi.restoreAllMocks();
  });
});

describe("bilingual", () => {
  it("stacks original above translation and keeps the link href", () => {
    const { host, unmount } = mount(
      <BlockView document={LIBRARY_FIXTURE} translated={LIBRARY_FIXTURE_EN} mode="bilingual" />,
    );
    const pairs = host.querySelectorAll(".block-pair");
    expect(pairs.length).toBeGreaterThan(0);
    const firstPair = pairs[0];
    expect(firstPair.querySelector(".block-orig")).not.toBeNull();
    expect(firstPair.querySelector(".block-tr")).not.toBeNull();
    expect(firstPair.firstElementChild?.className).toBe("block-orig");
    expect(firstPair.lastElementChild?.className).toBe("block-tr");
    expect(host.textContent).toContain("开馆日");
    expect(host.textContent).toContain("Opening Day");
    const link = host.querySelector("a") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("https://library.example/hours");
    unmount();
  });
});

describe("embeds", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.replaceChildren();
  });

  const embedDoc = {
    v: 1,
    blocks: [
      { t: "embed", kind: "youtube", url: "https://cdn.embedly.com/widgets/media.html?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3Dabc123def45", title: "https://cdn.embedly.com/widgets/media.html?youtube.com" },
      { t: "embed", kind: "iframe", url: "https://www.bilibili.com/video/BV1xx411c7mD/", title: "旧标题" },
      { t: "embed", kind: "youtube", url: "https://www.youtube.com/watch?v=abc123def45", title: "旧标题" },
      { t: "embed", kind: "youtube", url: "https://www.youtube.com/watch?v=remote", title: "远程封面" },
      { t: "embed", kind: "video", url: "https://cdn.example/a.mp4", title: "视频" },
      { t: "embed", kind: "video", url: "https://cdn.example/live.m3u8?token=1", title: "https://cdn.example/live.m3u8?token=1" },
    ],
  };
  const covers = [
    {
      url: "https://www.bilibili.com/video/BV1xx411c7mD/",
      open_url: "https://www.bilibili.com/video/BV1xx411c7mD/",
      site: "bilibili",
      title: "示例视频",
      author: "示例作者",
      duration_secs: 482,
      cover_href: "rrss://localhost/media/3",
      failed: false,
    },
    {
      url: "https://www.youtube.com/watch?v=abc123def45",
      open_url: "https://www.youtube.com/watch?v=abc123def45",
      site: "youtube",
      title: "芯片升级",
      author: "极客湾",
      duration_secs: 99,
      cover_href: "rrss://localhost/media/4",
      failed: false,
    },
    {
      url: "https://www.youtube.com/watch?v=remote",
      open_url: "https://www.youtube.com/watch?v=remote",
      site: "youtube",
      title: "不该用远程图",
      author: "某人",
      duration_secs: null,
      cover_href: "https://i.ytimg.com/vi/remote/hqdefault.jpg",
      failed: false,
    },
  ];

  it("shows cover copy and never puts a remote url on img video or audio", () => {
    const { host, unmount } = mount(
      <BlockView document={embedDoc} embedCovers={covers} articleId={7} onOpenUrl={() => {}} />,
    );
    const bili = host.querySelectorAll(".embed-cover")[0];
    expect(bili.querySelector(".embed-cover-title")?.textContent).toBe("示例视频");
    expect(bili.querySelector(".embed-cover-meta")?.textContent).toBe("哔哩哔哩 · 示例作者 · 8:02");
    const yt = [...host.querySelectorAll(".embed-cover")].find((el) => el.textContent?.includes("芯片升级"));
    expect(yt?.querySelector(".embed-cover-meta")?.textContent).toBe("YouTube · 极客湾");
    expect(yt?.textContent).not.toMatch(/\d+:\d+/);
    expect(host.textContent).not.toContain("不该用远程图");
    expect(host.textContent).not.toContain("watch?v=");
    expect(host.textContent).not.toContain("token=");
    expect(host.textContent).toContain("播放");
    expect(host.querySelector("video, audio")).toBeNull();
    for (const el of host.querySelectorAll("img, video, audio")) {
      const src = el.getAttribute("src") ?? "";
      expect(src.startsWith("rrss://") || src.startsWith("http://rrss.localhost/") || src.startsWith("https://rrss.localhost/")).toBe(true);
      expect(src).not.toContain("ytimg.com");
      expect(src).not.toContain("youtube.com");
      expect(src).not.toContain("cdn.example");
    }
    unmount();
  });

  it("starts local playback only after download and only with a media href", async () => {
    vi.spyOn(api, "downloadDirect").mockResolvedValue({
      src: "https://cdn.example/a.mp4",
      href: "rrss://localhost/media/8",
      width: null,
      height: null,
      error: "",
    });
    const fetched: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      fetched.push(String(input));
      const blob = new Blob([new Uint8Array([0, 1, 2])], { type: "video/mp4" });
      return { ok: true, blob: async () => blob };
    }));
    const previousCreate = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
    const previousRevoke = Object.getOwnPropertyDescriptor(URL, "revokeObjectURL");
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      writable: true,
      value: () => "blob:local-media",
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      writable: true,
      value: () => {},
    });
    const { host, unmount } = mount(
      <BlockView
        document={{ v: 1, blocks: [{ t: "embed", kind: "video", url: "https://cdn.example/a.mp4", title: "视频" }] }}
        articleId={7}
      />,
    );
    try {
      const button = [...host.querySelectorAll("button")].find((el) => el.textContent === "播放");
      expect(button).toBeTruthy();
      await act(async () => {
        button!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
        await new Promise((resolve) => setTimeout(resolve, 20));
      });
      expect(api.downloadDirect).toHaveBeenCalledWith(7, "https://cdn.example/a.mp4");
      expect(fetched).toEqual(["rrss://localhost/media/8"]);
      expect(host.querySelector("video")?.getAttribute("src")).toBe("blob:local-media");
      expect(host.textContent).not.toContain("cdn.example");
    } finally {
      unmount();
      if (previousCreate) Object.defineProperty(URL, "createObjectURL", previousCreate);
      else delete (URL as { createObjectURL?: unknown }).createObjectURL;
      if (previousRevoke) Object.defineProperty(URL, "revokeObjectURL", previousRevoke);
      else delete (URL as { revokeObjectURL?: unknown }).revokeObjectURL;
      vi.unstubAllGlobals();
    }
  });
});
