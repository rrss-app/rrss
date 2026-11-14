import { act, useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChatPane, mergeLatestPage, useConversationRuntime } from "../features/chat";
import { EMPTY_CONVERSATION, createConversationStore } from "../features/chat/conversationStore";
import type { ChatMessageView, CitationView, MessagesPage } from "../lib/api";
import { DigestDiscussion } from "../features/digest/DigestDiscussion";
import { createApiStub, type ApiStub } from "./api";
import { deferred } from "./deferred";
import { createEventStub, type EventStub } from "./events";
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

function message(id: number, text: string, citations: CitationView[] = []): ChatMessageView {
  return {
    id,
    role: "user",
    text,
    streaming: false,
    cite_article_id: citations[0]?.article_id ?? 0,
    cite_title: citations[0]?.title ?? "",
    citations,
    document: null,
  };
}

function page(rows: ChatMessageView[], hasMore: boolean): MessagesPage {
  return { messages: rows, has_more: hasMore };
}

const citation: CitationView = {
  kind: "article",
  article_id: 7,
  title: "引用标题",
  feed_title: "源",
  url: "https://example.invalid/7",
  excerpt: "摘录原文",
  block_id: 2,
};

describe("mergeLatestPage", () => {
  const older = [message(1, "更早"), message(2, "次早")];

  it("keeps already loaded older messages when the fresh page overlaps", () => {
    const merged = mergeLatestPage(
      { ...EMPTY_CONVERSATION, messages: [...older, message(3, "旧三")], hasMore: true },
      page([message(3, "新三"), message(4, "新四")], false),
    );
    expect(merged.messages.map((row) => row.id)).toEqual([1, 2, 3, 4]);
    expect(merged.messages[2].text).toBe("新三");
    expect(merged.hasMore).toBe(true);
  });

  it("replaces the window when the fresh page does not overlap, instead of inventing a gap", () => {
    const merged = mergeLatestPage(
      { ...EMPTY_CONVERSATION, messages: older, hasMore: true },
      page([message(9, "跳到这里"), message(10, "最新")], true),
    );
    expect(merged.messages.map((row) => row.text)).toEqual(["跳到这里", "最新"]);
    expect(merged.hasMore).toBe(true);
  });
});

function HistoryProbe() {
  const runtime = useConversationRuntime({ onError: () => {}, onDone: () => {} });
  useEffect(() => {
    void runtime.loadMessages(1);
  }, [runtime]);
  return (
    <ChatPane store={runtime.store} threadId={1} busy={false} onOpenUrl={() => {}} onLoadOlder={runtime.loadOlder} />
  );
}

describe("message history window", () => {
  const harness = createReactHarness();

  beforeEach(() => {
    box.stub = createApiStub();
    box.events = createEventStub();
    box.stub!.handle("messages", () => page([message(3, "最新")], true));
  });

  afterEach(() => {
    harness.cleanup();
    box.stub?.assertNoUnexpectedCalls();
  });

  async function open(node: React.ReactNode = <HistoryProbe />) {
    const mounted = harness.mount(node);
    await harness.flush();
    return mounted;
  }

  it("loads the latest page, then older pages without duplicates, and keeps citations", async () => {
    box.stub!.handle("messages", (id: number, before?: number | null) => {
      expect(id).toBe(1);
      if (before == null) return page([message(5, "第五"), message(6, "第六")], true);
      if (before === 5) return page([message(3, "第三", [citation]), message(4, "第四")], true);
      return page([message(1, "第一"), message(2, "第二")], false);
    });
    const { host } = await open();
    expect(host.textContent).toContain("加载更早的消息");
    expect(host.textContent).toContain("第六");
    expect(host.textContent).not.toContain("第四");

    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    await harness.flush();
    // 全文引用的小签只显示篇名（T2a），摘录不出现在小签上。
    expect(host.querySelector(".message-citations .citation-chip")).not.toBeNull();
    expect(host.textContent).toContain("引用标题");
    const once = [...host.querySelectorAll(".bubble")].map((row) => row.textContent);
    expect(once.filter((text) => text?.includes("第四"))).toHaveLength(1);

    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    await harness.flush();
    expect(host.querySelector(".chat-load-older")).toBeNull();
    expect([...host.querySelectorAll(".bubble")]).toHaveLength(6);
  });

  it("keeps the loaded window and the button when loading older history fails", async () => {
    const { host } = await open();
    box.stub!.handle("messages", () => {
      throw new Error("database locked");
    });
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    await harness.flush();
    expect(host.textContent).toContain("最新");
    expect(host.textContent).toContain("加载失败");
    expect(host.textContent).toContain("数据库正忙，请稍后重试");
    expect(host.querySelector(".chat-load-older button")?.hasAttribute("disabled")).toBe(false);
  });

  it("drops an older page that arrives after the window start has moved", async () => {
    const gate = deferred<MessagesPage>();
    let calls = 0;
    box.stub!.handle("messages", () => {
      calls += 1;
      if (calls === 1) return page([message(8, "当前起点")], true);
      if (calls === 2) return gate.promise;
      return page([message(20, "刷新后的起点"), message(21, "刷新后的最新")], false);
    });
    const { host } = await open();
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    act(() => box.events!.emit("chat:done", 1));
    await harness.flush();
    expect(host.textContent).toContain("刷新后的起点");
    await act(async () => {
      gate.resolve(page([message(1, "过期的更早消息")], false));
    });
    await harness.flush();
    expect(host.textContent).not.toContain("过期的更早消息");
    expect(host.textContent).toContain("刷新后的最新");
  });

  it("keeps the visible messages in place when older history is prepended", async () => {
    box.stub!.handle("messages", (_id: number, before?: number | null) =>
      before == null ? page([message(4, "可见")], true) : page([message(1, "补上的更早")], false),
    );
    const { host } = await open();
    const scroller = host.querySelector<HTMLDivElement>(".chat-pane")!;
    let height = 120;
    Object.defineProperty(scroller, "scrollHeight", { configurable: true, get: () => height });
    scroller.scrollTop = 30;
    box.stub!.handle("messages", () => {
      height = 200;
      return page([message(1, "补上的更早")], false);
    });
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    await harness.flush();
    expect(host.textContent).toContain("补上的更早");
    expect(scroller.scrollTop).toBe(110);
  });

  it("does not shift the scroll position when the older page is discarded", async () => {
    const late = deferred<MessagesPage>();
    let latestLoads = 0;
    box.stub!.handle("messages", (_id: number, before?: number | null) => {
      if (before == null) {
        latestLoads += 1;
        return latestLoads === 1 ? page([message(4, "可见")], true) : page([message(20, "刷新后")], false);
      }
      return late.promise;
    });
    const { host } = await open();
    const scroller = host.querySelector<HTMLDivElement>(".chat-pane")!;
    Object.defineProperty(scroller, "scrollHeight", { configurable: true, get: () => 120 });
    scroller.scrollTop = 30;
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    act(() => box.events!.emit("chat:done", 1));
    await harness.flush();
    await act(async () => late.resolve(page([message(1, "不该拼接")], false)));
    await harness.flush();
    expect(scroller.scrollTop).toBe(30);
    expect(host.textContent).not.toContain("不该拼接");
    expect(host.textContent).toContain("刷新后");
  });
});

describe("history release", () => {
  it("releases a settled thread and refuses one that is busy or still streaming", () => {
    const store = createConversationStore();
    store.update(1, () => ({ ...EMPTY_CONVERSATION, messages: [message(1, "已落库")] }));
    expect(store.release(1)).toBe(true);
    expect(store.get(1).messages).toEqual([]);

    store.update(2, () => ({ ...EMPTY_CONVERSATION, messages: [message(2, "未落库")], stream: { thread_id: 2, text: "半截回复", document: null } }));
    expect(store.release(2)).toBe(false);
    expect(store.get(2).stream?.text).toBe("半截回复");

    store.update(3, () => ({ ...EMPTY_CONVERSATION, messages: [message(3, "忙")] }));
    store.setStatus({ busyThreadId: 3 });
    expect(store.release(3)).toBe(false);
    expect(store.get(3).messages).toHaveLength(1);
  });
});

describe("digest follow-up history", () => {
  const harness = createReactHarness();

  beforeEach(() => {
    box.stub = createApiStub();
    box.events = createEventStub();
  });

  afterEach(() => {
    harness.cleanup();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("loads an older follow-up page for the current digest only", async () => {
    const store = createConversationStore();
    store.update(9, () => ({ ...EMPTY_CONVERSATION, messages: [message(6, "本期最新")], hasMore: true }));
    const seen: Array<number | null> = [];
    box.stub!.handle("messages", (id: number, before?: number | null) => {
      seen.push(before ?? null);
      expect(id).toBe(9);
      return page([message(2, "本期更早")], false);
    });
    const { host } = harness.mount(
      <DigestDiscussion
        store={store}
        threadId={9}
        context="digest:1"
        busy={false}
        scrollRequest={0}
        onLoadOlder={async (id) => {
          const page = await box.stub!.api.messages(id, store.get(id).messages[0]?.id ?? null);
          store.update(id, (current) => ({ ...current, messages: [...page.messages, ...current.messages], hasMore: page.has_more }));
          return true;
        }}
        onOpenUrl={() => {}}
        onOpenArticle={() => {}}
      />,
    );
    await harness.flush();
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".chat-load-older button")!.click();
    });
    await harness.flush();
    expect(seen).toEqual([6]);
    expect(host.textContent).toContain("本期更早");
    expect(host.textContent).toContain("本期最新");
    expect(host.querySelector(".chat-load-older")).toBeNull();
  });
});
