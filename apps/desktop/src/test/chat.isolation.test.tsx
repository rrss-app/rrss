import { threadFixture, digestThreadFixture } from "./fixtures/chat";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import type { ChatMessageView } from "../lib/api";
import { createApiStub, type ApiStub } from "./api";
import { createEventStub, type EventStub } from "./events";
import { clickText, handleShellReads, typeInto } from "./fixtures/shell";
import { createReactHarness } from "./react";

const box = vi.hoisted(() => ({
  stub: null as ApiStub | null,
  events: null as EventStub | null,
  focusRenders: 0,
  blockRenders: [] as string[],
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

vi.mock("../components/FocusPane", async () => {
  const actual = await vi.importActual<typeof import("../components/FocusPane")>("../components/FocusPane");
  const Counted = (props: Parameters<typeof actual.FocusPane>[0]) => {
    box.focusRenders += 1;
    return <actual.FocusPane {...props} />;
  };
  return { ...actual, FocusPane: Counted };
});

vi.mock("../components/BlockView", async () => {
  const actual = await vi.importActual<typeof import("../components/BlockView")>("../components/BlockView");
  const Counted = (props: Parameters<typeof actual.BlockView>[0]) => {
    box.blockRenders.push(JSON.stringify(props.document).slice(0, 60));
    return <actual.BlockView {...props} />;
  };
  return { ...actual, BlockView: Counted };
});

function message(id: number, role: "user" | "assistant", text: string): ChatMessageView {
  return {
    id,
    role,
    text,
    streaming: false,
    cite_article_id: 0,
    cite_title: "",
    citations: [],
    document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text }] }] },
  };
}

describe("conversation update isolation", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };

  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    box.stub = createApiStub();
    box.events = createEventStub();
    box.focusRenders = 0;
    box.blockRenders = [];
    handleShellReads(box.stub);
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
  });

  const delta = (thread_id: number, text: string) =>
    act(() => box.events!.emit("chat:delta", { thread_id, text, document: null }));

  it("does not re-render the focus view when a background thread streams", async () => {
    const { host } = harness.mount(<App />);
    await harness.flush();
    expect(host.textContent).toContain("甲文");
    // 开始 / 结束改变忙碌状态，根壳允许更新一次；正文快照不应再触达焦点视图。
    act(() => box.events!.emit("chat:start", 7));
    await harness.flush();
    const settled = box.focusRenders;
    for (let i = 0; i < 5; i++) {
      delta(7, `第 ${i} 段`);
      await harness.flush();
    }
    expect(box.focusRenders).toBe(settled);
    expect(host.textContent).not.toContain("第 4 段");
  });

  it("keeps history messages untouched while the current thread streams and other threads stay separate", async () => {
    box.stub!.handle("threadList", () => ({ folders: [], threads: [threadFixture({ id: 1, title: "线程一" }), threadFixture({ id: 2, title: "线程二" })], archived_count: 0 }));
    box.stub!.handle("messages", (id: number) => ({
      messages: id === 1
        ? [message(11, "user", "问一"), message(12, "assistant", "答一")]
        : [message(21, "user", "问二")],
      has_more: false,
    }));
    const { host } = harness.mount(<App />);
    await harness.flush();
    clickText(host, "线程一");
    await harness.flush();
    expect(host.textContent).toContain("答一");
    box.blockRenders = [];

    act(() => box.events!.emit("chat:start", 1));
    await harness.flush();
    delta(1, "答一续");
    await harness.flush();
    delta(1, "答一续写");
    await harness.flush();
    expect(host.textContent).toContain("答一续写");
    // 历史消息在流式期间没有重新遍历渲染。
    expect(box.blockRenders.filter((d) => d.includes("答一\""))).toEqual([]);

    delta(2, "串台内容");
    await harness.flush();
    expect(host.textContent).not.toContain("串台内容");
    clickText(host, "线程二");
    await harness.flush();
    expect(host.textContent).toContain("问二");
    expect(host.textContent).toContain("串台内容");
    expect(host.textContent).not.toContain("答一续写");
  });

  it("typing in the composer does not re-render the digest document", async () => {
    // 隔离输入更新与后台定时预载，先收敛初始化再记录正文渲染。
    const clock = harness.useClock();
    const issue = {
      id: 5,
      period: "daily",
      period_label: "日报",
      period_start: 0,
      date_label: "9月20日",
      body: "",
      revision: "isolation-digest-revision-5",
      source_state: "unavailable",
      source_note: "",
      source_index: [],
      unlocated_sources: [],
      items: [],
      document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "简报正文" }] }] },
    };
    box.stub!.handle("digests", () => [issue]);
    box.stub!.handle("digest", () => issue);
    box.stub!.handle("digestThread", () => (digestThreadFixture()));
    const { host } = harness.mount(<App />);
    await harness.flush();
    clickText(host, "简报");
    await harness.flush();
    await clock.advance(0);
    await harness.flush();
    expect(host.textContent).toContain("简报正文");
    box.blockRenders = [];
    const textarea = host.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    typeInto(textarea, "展开");
    typeInto(textarea, "展开第二条");
    await harness.flush();
    await clock.advance(0);
    expect(textarea.value).toBe("展开第二条");
    expect(box.blockRenders).toEqual([]);
  });
});
