import { describe, expect, it, vi } from "vitest";
import { EMPTY_CONVERSATION, createConversationStore } from "./conversationStore";
import { createDraftStore, EMPTY_DRAFT } from "./draftStore";
import { createTurnStore } from "./turnTrace";

describe("conversation store", () => {
  it("notifies only the updated thread and keeps untouched snapshots stable", () => {
    const store = createConversationStore();
    const one = vi.fn();
    const two = vi.fn();
    store.subscribe(1, one);
    const unsubscribeTwo = store.subscribe(2, two);
    expect(store.get(1)).toBe(EMPTY_CONVERSATION);

    store.update(1, (cur) => ({ ...cur, stream: { thread_id: 1, text: "a", document: null } }));
    expect(one).toHaveBeenCalledTimes(1);
    expect(two).not.toHaveBeenCalled();
    expect(store.get(2)).toBe(EMPTY_CONVERSATION);

    const before = store.get(1);
    store.update(1, (cur) => cur);
    expect(one).toHaveBeenCalledTimes(1);
    expect(store.get(1)).toBe(before);

    unsubscribeTwo();
    store.update(2, (cur) => ({ ...cur, stream: { thread_id: 2, text: "b", document: null } }));
    expect(two).not.toHaveBeenCalled();
  });

  it("publishes status changes once per real change", () => {
    const store = createConversationStore();
    const listener = vi.fn();
    store.subscribeStatus(listener);
    store.setStatus({ busyThreadId: 3 });
    store.setStatus({ busyThreadId: 3 });
    store.setStatus({ sending: true });
    expect(listener).toHaveBeenCalledTimes(2);
    expect(store.getStatus()).toEqual({ busyThreadId: 3, sending: true });
  });
});

describe("draft store", () => {
  it("moves drafts without overwriting a target and only consumes the accepted snapshot", () => {
    const store = createDraftStore();
    const full = { kind: "article", article_id: 1, title: "甲文", feed_title: "", url: "", excerpt: "", block_id: 0 };
    const span = { ...full, kind: "span", excerpt: "一段原句" };
    store.setText("chat", "当前问题");
    store.addCitations("chat", [full, span]);
    store.setText(store.threadContext(8), "已有草稿");
    store.addCitations(store.threadContext(8), [{ ...full }]);
    store.move("chat", store.threadContext(8));
    expect(store.get("chat")).toBe(EMPTY_DRAFT);
    expect(store.get(store.threadContext(8))).toMatchObject({ text: "已有草稿\n当前问题", citations: [full, span] });
    store.bindThread(8, "digest:5");
    expect(store.threadContext(8)).toBe("digest:5");
    expect(store.get("chat:8")).toBe(EMPTY_DRAFT);
    const sent = store.get("digest:5");
    const later = { ...span, article_id: 2 };
    store.addCitations("digest:5", [later]);
    store.setText("digest:5", "发送期间的新问题");
    store.consume("digest:5", sent);
    expect(store.get(store.threadContext(8))).toEqual({ text: "发送期间的新问题", citations: [later] });
    expect(store.threadContext(0)).toBe("chat");
    expect(store.threadContext(9)).toBe("chat:9");
  });
  it("puts a failed send back ahead of what was typed meanwhile, without duplicating citations", () => {
    const drafts = createDraftStore();
    const cite = { kind: "article", article_id: 3, title: "甲", feed_title: "", url: "", excerpt: "", block_id: -1 };
    const later = { ...cite, article_id: 4, title: "乙" };
    drafts.setText("chat", "发出的问题");
    drafts.addCitations("chat", [cite]);
    const sent = drafts.get("chat");
    drafts.consume("chat", sent);
    expect(drafts.get("chat")).toEqual(EMPTY_DRAFT);
    drafts.setText("chat", "后来打的");
    drafts.addCitations("chat", [later]);
    drafts.restore("chat", sent);
    expect(drafts.get("chat").text).toBe("发出的问题\n后来打的");
    expect(drafts.get("chat").citations).toEqual([cite, later]);
  });

  it("keeps drafts per context and only updates the context that was written", () => {
    const store = createDraftStore();
    const chat = vi.fn();
    const digest = vi.fn();
    store.subscribe("chat", chat);
    store.subscribe("digest:5", digest);
    expect(store.get("chat")).toBe(EMPTY_DRAFT);

    store.setText("chat", "你好");
    store.addCitations("digest:5", [{ kind: "article", article_id: 1, title: "甲文", feed_title: "", url: "", excerpt: "", block_id: 0 }]);
    expect(chat).toHaveBeenCalledTimes(1);
    expect(digest).toHaveBeenCalledTimes(1);
    expect(store.get("chat")).toEqual({ text: "你好", citations: [] });
    expect(store.get("digest:5").citations).toHaveLength(1);

    store.clear("chat");
    expect(store.get("chat")).toBe(EMPTY_DRAFT);
    expect(store.get("digest:5").citations).toHaveLength(1);
    store.clear("chat");
    expect(chat).toHaveBeenCalledTimes(2);
  });
});

describe("turn store · external tools", () => {
  const mcp = (start: boolean) => ({ kind: "tool", id: "m1", start, tool: "mcp__s1__write_file", server: "filesystem", server_tool: "write_file",
    label: "使用 filesystem 的 write_file", detail: "", article_id: null, error: false });

  it("records the answer on the step; a denied call ends there and a late end is ignored", () => {
    let clock = 0;
    const turns = createTurnStore(() => clock);
    turns.begin("t7", 7);
    turns.step(mcp(true), 7);
    turns.ask(7, "filesystem", "write_file");
    expect(turns.getLive()?.asking).toEqual({ server: "filesystem", tool: "write_file" });
    clock = 900;
    turns.answer(7, "filesystem", "write_file", false);
    const step = turns.getLive()!.steps[0];
    expect(turns.getLive()?.asking).toBeNull();
    expect(step).toMatchObject({ approval: "denied", endedAt: 900 });
    clock = 1500;
    turns.step({ ...mcp(false), error: true }, 7);
    expect(turns.getLive()!.steps[0]).toMatchObject({ endedAt: 900, error: false });
    turns.answering(7, 4);
    turns.finish(7, 42);
    expect(turns.getTraces().get(42)).toMatchObject({ tools: 0, external: 1 });
  });

  it("times a tool called after the reply started, until the reply goes on", () => {
    let clock = 0;
    const turns = createTurnStore(() => clock);
    turns.begin("t7", 7);
    clock = 2000;
    turns.answering(7, 6);
    clock = 3000;
    turns.step(mcp(true), 7);
    expect(turns.getLive()?.pauses).toEqual([{ from: 3000, to: null }]);
    clock = 8000;
    turns.step(mcp(false), 7);
    // 一条消息结束时同样的文本再来一次，不算接着出字。
    clock = 8500;
    turns.answering(7, 6);
    expect(turns.getLive()?.pauses.at(-1)?.to).toBeNull();
    clock = 9000;
    turns.answering(7, 20);
    expect(turns.getLive()?.pauses).toEqual([{ from: 3000, to: 9000 }]);
    clock = 20000;
    turns.finish(7, 42);
    const trace = turns.getTraces().get(42)!;
    // 出字前 2 秒 + 调用到接着出字 6 秒；出字本身不算。
    expect(trace.elapsed).toBe(8000);
    expect(trace.external).toBe(1);
    expect(trace.steps.map((step) => [step.kind, step.startedAt, step.endedAt])).toEqual([
      ["think", 0, 2000], ["tool", 3000, 8000], ["think", 8000, 9000],
    ]);
  });

  it("keeps an answer that arrives before its step", () => {
    const turns = createTurnStore(() => 0);
    turns.begin("t7", 7);
    turns.answer(7, "filesystem", "write_file", true);
    turns.step(mcp(true), 7);
    expect(turns.getLive()!.steps[0]).toMatchObject({ approval: "allowed", endedAt: null });
  });
});
