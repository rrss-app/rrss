import { deferred } from "../test/deferred";
import { act } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { createDraftStore, type useConversationRuntime } from "../features/chat";
import { createReactHarness } from "../test/react";
import { useReaderCitations } from "./useReaderCitations";

const harness = createReactHarness();
afterEach(() => harness.cleanup());
function setup() {
  let pending: ReturnType<typeof deferred<number>>;
  const sendMessage = vi.fn(() => { pending = deferred<number>(); return pending.promise; });
  const chat = { sendMessage, seedThread: vi.fn(), sending: false, busyThreadId: null } as unknown as ReturnType<typeof useConversationRuntime>;
  const drafts = createDraftStore(), onSent = vi.fn();
  let article = { id: 1, title: "发送时的文章" };
  let adapter: ReturnType<typeof useReaderCitations>;
  function Probe() {
    adapter = useReaderCitations({ drafts, chat, getThreads: () => [], getSourceArticle: () => article, onSent, notify: vi.fn(), inert: false });
    return null;
  }
  harness.mount(<Probe />);
  act(() => { adapter.begin({ kind: "thread", threadId: 9, title: "目标会话" }); adapter.ask(); });
  return { drafts, get adapter() { return adapter!; }, onSent, finish: (id: number) => pending.resolve(id), setArticle: (next: typeof article) => { article = next; } };
}
it("records the article at send time and overwrites only that thread after another successful send", async () => {
  const context = setup();
  let sent!: Promise<boolean>;
  act(() => { sent = context.adapter.tray!.props.onSend("问题", []); });
  context.setArticle({ id: 2, title: "等待期间换篇" });
  await act(async () => { context.finish(9); await sent; });
  expect(context.adapter.origins.get(9)).toEqual({ id: 1, title: "发送时的文章" });
  act(() => context.adapter.ask());
  act(() => { sent = context.adapter.tray!.props.onSend("继续", []); });
  await act(async () => { context.finish(9); await sent; });
  expect([...context.adapter.origins]).toEqual([[9, { id: 2, title: "等待期间换篇" }]]);
});
it("does not attach a late send to a reopened reader", async () => {
  const context = setup();
  let sent!: Promise<boolean>;
  act(() => { sent = context.adapter.tray!.props.onSend("问题", []); });
  act(() => { context.adapter.end(); context.adapter.begin({ kind: "new" }); });
  await act(async () => { context.finish(9); await sent; });
  expect(context.adapter.origins.size).toBe(0);
  expect(context.onSent).toHaveBeenCalledWith(9, null, false);
});

it("captures the tray after begin and rejects a quote that arrives after leaving", () => {
  const context = setup();
  const chip = { kind: "article", article_id: 10, title: "引用目标", feed_title: "", url: "https://example.test/story", excerpt: "", block_id: 0 };
  let add!: ReturnType<typeof context.adapter.captureAdd>;
  act(() => { context.adapter.begin({ kind: "new" }); add = context.adapter.captureAdd(); });
  act(() => add(chip));
  expect(context.drafts.get(context.drafts.threadContext(0)).citations).toEqual([chip]);
  act(() => { context.adapter.end(); context.adapter.begin({ kind: "thread", threadId: 12, title: "另一会话" }); });
  act(() => add({ ...chip, article_id: 11 }));
  expect(context.drafts.get(context.drafts.threadContext(12)).citations).toEqual([]);
});
