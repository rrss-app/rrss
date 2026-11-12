import { digestThreadFixture, threadFixture } from "./test/fixtures/chat";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { api, type ChatMessageView, type ChatStreamEvent, type CitationView, type DigestThreadView } from "./lib/api";
import { PREVIEW_DIGESTS, PREVIEW_FEEDS, PREVIEW_FOCUS } from "./lib/preview";
import { sourceLabel } from "./components/DigestSourcePreview";

const listeners = vi.hoisted(() => new Map<string, (payload: never) => void>());
vi.mock("./lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./lib/api")>();
  const listen = (name: string) => (cb: (payload: never) => void) => {
    listeners.set(name, cb);
    return Promise.resolve(() => listeners.delete(name));
  };
  return {
    ...actual,
    api: {
      ...actual.api,
      focus: vi.fn(), feeds: vi.fn(), threads: vi.fn(), threadList: vi.fn(), archivedThreads: vi.fn(), messages: vi.fn(), digests: vi.fn(), digest: vi.fn(),
      digestThread: vi.fn(), sendMessage: vi.fn(), reader: vi.fn(), openArticle: vi.fn(), newThread: vi.fn(), abort: vi.fn(), citeDigestSelection: vi.fn(),
    },
    onChatStart: listen("start"), onChatDelta: listen("delta"), onChatDone: listen("done"),
    onChatError: listen("error"), onMediaReady: async () => () => {},
  };
});

const daily = PREVIEW_DIGESTS.filter((d) => d.period === "daily");
const latest = daily[0];
const previous = daily[1];
const digestCitations: CitationView[] = latest.items[0].sources.map(({ article_id, title, feed_title }) => ({
  article_id, title, feed_title, kind: "digest", excerpt: "列表项", url: "https://example.test", block_id: -1,
}));
const message = (id: number, text: string, role = "assistant"): ChatMessageView => ({
  id, text, role, streaming: false, cite_article_id: 0, cite_title: "", citations: [], document: null,
});
const stream = (thread_id: number, text: string): ChatStreamEvent => ({ thread_id, text, document: null });

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

let host: HTMLDivElement;
let root: Root;
async function flush() { await act(async () => { await Promise.resolve(); }); }
async function mount() {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(<App />));
}
async function click(name: string) {
  const button = [...host.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === name || b.getAttribute("aria-label") === name);
  expect(button, name).toBeTruthy();
  await act(async () => button!.click());
}
function type(text: string) {
  const textarea = host.querySelector("textarea")!;
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(textarea, text);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  });
  return textarea;
}
async function emit(name: string, payload: unknown) {
  await act(async () => listeners.get(name)?.(payload as never));
}
async function quoteCurrentItem() {
  const block = host.querySelector('.digest-doc [data-block-id="2"]')!;
  const range = document.createRange();
  range.selectNodeContents(block);
  window.getSelection()?.removeAllRanges();
  window.getSelection()?.addRange(range);
  await act(async () => block.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 40, clientY: 100 })));
  const quote = document.querySelector<HTMLButtonElement>('.sel-menu [role="menuitem"]:last-child');
  expect(quote).not.toBeNull();
  await act(async () => quote!.click());
}

beforeAll(() => { (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true; });
beforeEach(() => {
  vi.resetAllMocks();
  listeners.clear();
  vi.mocked(api.focus).mockResolvedValue(PREVIEW_FOCUS);
  vi.mocked(api.feeds).mockResolvedValue(PREVIEW_FEEDS);
  vi.mocked(api.threads).mockResolvedValue([]);
  vi.mocked(api.threadList).mockResolvedValue({ folders: [], threads: [], archived_count: 0 });
  vi.mocked(api.archivedThreads).mockResolvedValue([]);
  vi.mocked(api.messages).mockResolvedValue({ messages: [], has_more: false });
  vi.mocked(api.digests).mockImplementation(async (period) => PREVIEW_DIGESTS.filter((d) => d.period === period));
  vi.mocked(api.digest).mockImplementation(async (id) => {
    const row = PREVIEW_DIGESTS.find((d) => d.id === id);
    if (!row) throw new Error("digest not found");
    return row;
  });
  vi.mocked(api.digestThread).mockResolvedValue(digestThreadFixture());
  vi.mocked(api.sendMessage).mockResolvedValue(41);
  vi.mocked(api.citeDigestSelection).mockResolvedValue(digestCitations);
  vi.mocked(api.openArticle).mockImplementation(async (id) => ({
    id, source_hash: `digest-${id}`, title: "第二条对应原文", meta: "测试源", feed_icon_href: "", url: "https://example.test/article", summary: "", score: 0,
    feedback: 0, document: { v: 1, blocks: [] }, reasons: [], media: [], translated_title: "",
    translated_document: null, translation_ready: false, translation_note: "", truncated: false, pruned: false, summary_stale: false, starred: false,
    embed_covers: [],
  }));
});
afterEach(() => {
  if (root) act(() => root.unmount());
  host?.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Digest 页内追问", () => {
  it("划词关联多个来源，翻期保留各自草稿，发送后仍能打开每个引用", async () => {
    await mount();
    await click("简报");
    await quoteCurrentItem();
    expect(api.citeDigestSelection).toHaveBeenCalledWith(latest.id, latest.revision, [{ blockId: 2, excerpt: "列表项" }]);
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(2);
    expect(host.querySelector("textarea")!.value).toBe("");
    expect(host.querySelector(".composer .citation-excerpt")!.textContent).toBe("“列表项”");
    type("为什么这条重要？");
    await click("上期");
    expect(host.querySelector("textarea")!.value).toBe("");
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(0);
    type("上期的问题");
    await click("下期");
    expect(host.querySelector("textarea")!.value).toBe("为什么这条重要？");
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(2);
    vi.mocked(api.messages).mockResolvedValue({ messages: [{ ...message(1, "为什么这条重要？", "user"), citations: digestCitations }], has_more: false });
    await click("发送");
    expect(api.sendMessage).toHaveBeenCalledWith(0, "为什么这条重要？", digestCitations, latest.id, null, null);
    expect(host.querySelectorAll(".digest-discussion .citation-chip")).toHaveLength(2);
    await click(`打开引用《${digestCitations[1].title}》`);
    expect(api.openArticle).toHaveBeenLastCalledWith(digestCitations[1].article_id);
    await click("返回");
    await click("上期");
    expect(host.querySelector("textarea")!.value).toBe("上期的问题");
    expect(host.querySelector(".digest")).not.toBeNull();
  });

  it("引用响应迟到且已经翻期时丢弃，不污染任一期草稿", async () => {
    const pending = deferred<CitationView[]>();
    vi.mocked(api.citeDigestSelection).mockReturnValue(pending.promise);
    await mount();
    await click("简报");
    await quoteCurrentItem();
    type("本期问题");
    await click("发送");
    expect(api.sendMessage).not.toHaveBeenCalled();
    await click("上期");
    type("另一份草稿");
    await act(async () => pending.resolve(digestCitations));
    expect(host.querySelector("textarea")!.value).toBe("另一份草稿");
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(0);
    await click("下期");
    expect(host.querySelector("textarea")!.value).toBe("本期问题");
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(0);
  });

  it("同一期重生成后即使 id 相同，迟到的旧 revision 引用也不会写入草稿", async () => {
    const pending = deferred<CitationView[]>();
    let refreshed = false;
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    vi.mocked(api.citeDigestSelection).mockReturnValue(pending.promise);
    vi.mocked(api.digest).mockImplementation(async (id) => {
      const row = PREVIEW_DIGESTS.find((item) => item.id === id);
      if (!row) throw new Error("digest not found");
      return refreshed && id === latest.id ? { ...row, revision: "new-revision-same-body", source_index: [row.source_index[1]] } : row;
    });
    vi.spyOn(api, "generateDigest").mockImplementation(async () => { refreshed = true; });
    await mount();
    await click("简报");
    await quoteCurrentItem();
    expect(api.citeDigestSelection).toHaveBeenCalledWith(latest.id, latest.revision, [{ blockId: 2, excerpt: "列表项" }]);
    await click("生成本期");
    await flush();
    expect(vi.mocked(api.digest).mock.calls.length).toBeGreaterThan(1);
    expect(api.digest).toHaveBeenLastCalledWith(latest.id);
    await act(async () => pending.resolve(digestCitations));
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(0);
    vi.mocked(api.citeDigestSelection).mockResolvedValue(digestCitations);
    await quoteCurrentItem();
    expect(api.citeDigestSelection).toHaveBeenLastCalledWith(latest.id, "new-revision-same-body", [{ blockId: 2, excerpt: "列表项" }]);
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(2);
  });

  it("无法映射来源时展示错误并保留问题，不伪造引用", async () => {
    vi.mocked(api.citeDigestSelection).mockRejectedValue(new Error("所选简报条目没有可用来源"));
    await mount();
    await click("简报");
    type("保留这个问题");
    await quoteCurrentItem();
    expect(host.textContent).toContain("所选简报条目没有可用来源");
    expect(host.querySelector("textarea")!.value).toBe("保留这个问题");
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(0);
  });

  it("发送绑定可见简报，流式快照不重复累加，回复链接返回同一期", async () => {
    vi.mocked(api.messages).mockResolvedValue({ messages: [message(1, "展开第二条", "user")], has_more: false });
    await mount();
    await click("简报");
    type("展开第二条");
    await click("发送");
    expect(api.sendMessage).toHaveBeenCalledWith(0, "展开第二条", [], latest.id, null, null);
    expect(host.querySelector(".digest")).not.toBeNull();
    expect(host.querySelector(".chat-pane")).toBeNull();
    expect(host.querySelector("textarea")!.value).toBe("");
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("展开第二条");
    await emit("delta", stream(41, "第二条"));
    await emit("delta", stream(41, "第二条完整回复"));
    expect(host.querySelector(".digest-discussion .assistant")!.textContent).toBe("第二条完整回复");
    const reply = {
      ...message(2, "第二条原文"),
      document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: "rrss://article/2", children: [{ t: "t", text: "阅读第二条原文" }] }] }] },
    };
    vi.mocked(api.messages).mockResolvedValue({ messages: [message(1, "展开第二条", "user"), reply], has_more: false });
    await emit("done", 41);
    await act(async () => host.querySelector<HTMLAnchorElement>(".digest-discussion a")!.click());
    expect(api.openArticle).toHaveBeenCalledWith(2);
    expect(host.querySelector(".drawer.reader")).not.toBeNull();
    await click("返回");
    expect(host.querySelector(".drawer.reader")).toBeNull();
    expect(host.querySelector(".digest")).not.toBeNull();
    expect(host.querySelector(".digest-date")!.textContent).toBe(latest.date_label);
  });

  it("翻到上期时不显示其他期的流式回复，返回后恢复对应记录", async () => {
    vi.mocked(api.digestThread).mockImplementation(async (id) => id === latest.id
      ? digestThreadFixture({ thread_id: 41 })
      : digestThreadFixture({ thread_id: 40, messages: [message(3, "上期独有回复")] }));
    await mount();
    await click("简报");
    await emit("start", 41);
    await click("上期");
    await emit("delta", stream(41, "本期独有回复"));
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("上期独有回复");
    expect(host.querySelector(".digest-discussion")!.textContent).not.toContain("本期独有回复");
    vi.mocked(api.messages).mockResolvedValue({ messages: [message(9, "本期独有回复")], has_more: false });
    await emit("done", 41);
    expect(host.querySelector(".digest-date")!.textContent).toBe(previous.date_label);
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("上期独有回复");
    await click("下期");
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("本期独有回复");
    expect(host.querySelector(".digest-discussion")!.textContent).not.toContain("上期独有回复");
  });

  it("追问上期复用该期线程，不挪用普通深度对话", async () => {
    vi.mocked(api.threadList).mockResolvedValue({ folders: [], threads: [threadFixture({ id: 8, title: "普通会话" })], archived_count: 0 });
    vi.mocked(api.digestThread).mockImplementation(async (id) => (digestThreadFixture({ thread_id: id === previous.id ? 40 : 0 })));
    vi.mocked(api.sendMessage).mockResolvedValue(40);
    await mount();
    await click("普通会话");
    await click("简报");
    await click("上期");
    type("展开第二条");
    await click("发送");
    expect(api.sendMessage).toHaveBeenCalledWith(40, "展开第二条", [], previous.id, null, "");
    await click("深度对话");
    expect(host.querySelector('.thread-item[data-on="1"]')!.textContent).toBe("普通会话");
    await click("简报");
    expect(host.querySelector(".digest-date")!.textContent).toBe(previous.date_label);
    expect(api.newThread).not.toHaveBeenCalled();
  });

  it("中文输入法确认不发送，发送失败保留草稿并允许重试", async () => {
    vi.mocked(api.sendMessage).mockRejectedValueOnce(new Error("模型未配置"));
    await mount();
    await click("简报");
    await click(sourceLabel(latest.items[0].sources[0]));
    await click("引用全文到对话");
    expect(host.querySelector(".reader-citation-tray .reader-citation-chip")).not.toBeNull();
    expect(host.querySelector(".reader-citation-target")!.textContent).toContain("本期追问");
    await click("返回");
    const textarea = type("展开第二条");
    act(() => textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", isComposing: true, bubbles: true })));
    await flush();
    expect(api.sendMessage).not.toHaveBeenCalled();
    await click("发送");
    expect(textarea.value).toBe("展开第二条");
    expect(host.querySelector(".composer .chip")).not.toBeNull();
    expect(host.textContent).toContain("模型未配置");
    expect(host.querySelector(".digest")).not.toBeNull();
    await click("发送");
    expect(api.sendMessage).toHaveBeenCalledTimes(2);
    expect(textarea.value).toBe("");
  });

  it("会话加载中禁发，迟到的旧期请求不覆盖当前期", async () => {
    const slow = deferred<DigestThreadView>();
    vi.mocked(api.digestThread).mockImplementation((id) => id === latest.id ? slow.promise : Promise.resolve(digestThreadFixture({ thread_id: 40, messages: [message(2, "上期记录")] })));
    await mount();
    await click("简报");
    type("展开第二条");
    await click("发送");
    expect(api.sendMessage).not.toHaveBeenCalled();
    await click("上期");
    await act(async () => slow.resolve(digestThreadFixture({ thread_id: 41, messages: [message(5, "不该出现的记录")] })));
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("上期记录");
    expect(host.querySelector(".digest-discussion")!.textContent).not.toContain("不该出现的记录");
    // 翻期后问题与引用各自保存；在上期明确输入问题后才发送。
    type("展开第二条");
    await click("发送");
    expect(api.sendMessage).toHaveBeenCalledWith(40, "展开第二条", [], previous.id, null, "");
  });

  it("回复落库失败时保留已收到的文本，而不是在刷新后丢失", async () => {
    vi.mocked(api.digestThread).mockResolvedValue(digestThreadFixture({ thread_id: 41, messages: [message(1, "展开第二条", "user")] }));
    vi.mocked(api.messages).mockResolvedValue({ messages: [message(1, "展开第二条", "user")], has_more: false });
    await mount();
    await click("简报");
    await emit("start", 41);
    await emit("delta", stream(41, "这段回复仍可复制"));
    await emit("error", { thread_id: 41, message: "回复保存失败" });
    await emit("done", 41);
    expect(host.querySelector(".digest-discussion .assistant")!.textContent).toBe("这段回复仍可复制");
    expect(host.textContent).toContain("回复保存失败");
  });

  it("空简报不发送，也不创建追问线程", async () => {
    vi.mocked(api.digests).mockResolvedValue([]);
    await mount();
    await click("简报");
    type("展开第二条");
    await click("发送");
    expect(api.sendMessage).not.toHaveBeenCalled();
    expect(api.digestThread).not.toHaveBeenCalled();
    expect(api.newThread).not.toHaveBeenCalled();
    expect(host.textContent).toContain("打开一期简报后即可追问");
  });

  it("历史加载失败时显示重试，并阻止误发到新线程", async () => {
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    vi.mocked(api.digestThread).mockRejectedValueOnce(new Error("database busy"));
    await mount();
    await click("简报");
    type("继续上次的问题");
    await click("发送");
    expect(api.sendMessage).not.toHaveBeenCalled();
    expect(host.textContent).toContain("追问记录加载失败");
    await click("重试加载追问");
    await click("发送");
    expect(api.sendMessage).toHaveBeenCalledWith(0, "继续上次的问题", [], latest.id, null, null);
  });

  it("发送等待期间翻期，不会在成功后被切回或绑定到错误期", async () => {
    const send = deferred<number>();
    vi.mocked(api.sendMessage).mockReturnValue(send.promise);
    vi.mocked(api.digestThread).mockImplementation(async (id) => (digestThreadFixture({ thread_id: id === previous.id ? 40 : 0, messages: id === previous.id ? [message(8, "上期内容")] : [] })));
    await mount();
    await click("简报");
    type("展开第二条");
    await click("发送");
    await click("上期");
    await act(async () => send.resolve(41));
    expect(host.querySelector(".digest-date")!.textContent).toBe(previous.date_label);
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("上期内容");
    expect(host.querySelector(".chat-pane")).toBeNull();
  });
});

describe("主栏目保活", () => {
  const hidden = (el: Element | null) => !!el?.closest("[hidden]");

  it("没打开过的简报 / 对话不请求；打开后切走只隐藏，切回不重拉", async () => {
    await mount();
    expect(api.digests).not.toHaveBeenCalled();
    expect(api.digest).not.toHaveBeenCalled();
    expect(api.digestThread).not.toHaveBeenCalled();
    expect(api.messages).not.toHaveBeenCalled();
    expect(host.querySelector(".digest")).toBeNull();
    expect(host.querySelector(".chat-pane")).toBeNull();
    const focus = host.querySelector(".focus")!;
    await click("简报");
    expect(api.digestThread).toHaveBeenCalledTimes(1);
    expect(hidden(focus)).toBe(true);
    const digest = host.querySelector(".digest")!;
    await click("今日焦点");
    expect(host.querySelector(".focus")).toBe(focus);
    expect(hidden(focus)).toBe(false);
    expect(hidden(digest)).toBe(true);
    expect(host.querySelector(".chat-pane")).toBeNull();
    await click("简报");
    expect(host.querySelector(".digest")).toBe(digest);
    expect(hidden(digest)).toBe(false);
    expect(api.digests).toHaveBeenCalledTimes(1);
    expect(api.digestThread).toHaveBeenCalledTimes(1);
    expect(api.messages).not.toHaveBeenCalled();
  });

  it("隐藏期间到达的追问记录与引用仍归属原简报", async () => {
    const thread = deferred<DigestThreadView>();
    const cite = deferred<CitationView[]>();
    vi.mocked(api.digestThread).mockReturnValue(thread.promise);
    vi.mocked(api.citeDigestSelection).mockReturnValue(cite.promise);
    await mount();
    await click("简报");
    await click("今日焦点");
    await act(async () => thread.resolve(digestThreadFixture({ thread_id: 50, messages: [message(5, "之前的追问", "user")] })));
    await click("简报");
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("之前的追问");
    expect(api.digestThread).toHaveBeenCalledTimes(1);
    await quoteCurrentItem();
    await click("今日焦点");
    await act(async () => cite.resolve(digestCitations));
    await click("简报");
    expect(host.querySelectorAll(".composer .citation-chip")).toHaveLength(2);
    expect(host.querySelector(".digest-discussion")!.textContent).toContain("之前的追问");
  });

  it("切到简报不回收当前对话，期间完成的回复留在原线程", async () => {
    const question = message(1, "问一个问题", "user");
    vi.mocked(api.messages).mockResolvedValue({ messages: [question], has_more: false });
    await mount();
    await click("深度对话");
    type("问一个问题");
    await click("发送");
    const pane = host.querySelector(".chat-pane")!;
    await emit("start", 41);
    await emit("delta", stream(41, "回复中"));
    await click("简报");
    expect(hidden(pane)).toBe(true);
    vi.mocked(api.messages).mockResolvedValue({ messages: [question, message(2, "完整回复")], has_more: false });
    await emit("done", 41);
    await click("深度对话");
    expect(host.querySelector(".chat-pane")).toBe(pane);
    expect(hidden(pane)).toBe(false);
    expect(pane.textContent).toContain("完整回复");
    const reads = vi.mocked(api.messages).mock.calls.length;
    await click("简报");
    await click("深度对话");
    expect(pane.textContent).toContain("问一个问题");
    expect(pane.textContent).toContain("完整回复");
    expect(api.messages).toHaveBeenCalledTimes(reads);
  });
});
