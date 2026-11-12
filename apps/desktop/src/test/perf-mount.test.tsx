import { Profiler, StrictMode, act, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useConversation, useConversationRuntime } from "../features/chat";
import { createApiStub, type ApiStub } from "./api";
import { createEventStub, type EventStub } from "./events";
import { createReactHarness } from "./react";

const box = vi.hoisted(() => ({
  stub: null as ApiStub | null,
  events: null as EventStub | null,
}));

vi.mock("../lib/api", async () => {
  const api = await import("./api");
  const events = await import("./events");
  return {
    api: api.createApiProxy(() => box.stub),
    ...events.eventListenerExports(() => box.events),
  };
});

type ProfilerCounts = { mounts: number; updates: number };

type CountReport = {
  profilerMountCommitsAfterStartFlush: number;
  profilerUpdateCommitsAfterStartFlush: number;
  eventSubscribeCount: number;
  chatStartUpdateCommits: number;
  chatDelta1UpdateCommits: number;
  chatDelta2UpdateCommits: number;
  chatDelta3UpdateCommits: number;
  chatDoneUpdateCommits: number;
};

/** 订阅被推流的线程，计数覆盖“运行层 + 一个订阅者”的更新提交。 */
function Probe() {
  const { store } = useConversationRuntime({ onError: () => {}, onDone: () => {} });
  const conversation = useConversation(store, 1001);
  return <span>{conversation.stream?.text ?? "conversations"}</span>;
}

function Profiled({ counts, children }: { counts: ProfilerCounts; children: ReactNode }) {
  return (
    <Profiler
      id="f0-2-conversations"
      onRender={(_id, phase) => {
        if (phase === "mount") counts.mounts += 1;
        if (phase === "update") counts.updates += 1;
      }}
    >
      {children}
    </Profiler>
  );
}

function reset(counts: ProfilerCounts) {
  counts.mounts = 0;
  counts.updates = 0;
}

describe("conversation runtime mount and stream update counts", () => {
  const harness = createReactHarness();

  function installStub() {
    box.stub = createApiStub();
    box.events = createEventStub();
    box.stub.handle("messages", async () => ({ messages: [], has_more: false }));
  }

  beforeEach(() => {
    installStub();
  });

  afterEach(() => {
    harness.cleanup();
    expect(box.events?.activeCount()).toBe(0);
    box.stub?.assertNoUnexpectedCalls();
  });

  async function run(wrap: (node: ReactNode) => ReactNode): Promise<CountReport> {
    const counts: ProfilerCounts = { mounts: 0, updates: 0 };
    const subscribeBefore = box.events!.subscribeCount;
    harness.mount(wrap(<Profiled counts={counts}><Probe /></Profiled>));
    await harness.flush();
    const afterStart: CountReport = {
      profilerMountCommitsAfterStartFlush: counts.mounts,
      profilerUpdateCommitsAfterStartFlush: counts.updates,
      eventSubscribeCount: box.events!.subscribeCount - subscribeBefore,
      chatStartUpdateCommits: 0,
      chatDelta1UpdateCommits: 0,
      chatDelta2UpdateCommits: 0,
      chatDelta3UpdateCommits: 0,
      chatDoneUpdateCommits: 0,
    };
    reset(counts);

    act(() => {
      box.events!.emit("chat:start", 1001);
    });
    await harness.flush();
    afterStart.chatStartUpdateCommits = counts.updates;
    reset(counts);

    act(() => {
      box.events!.emit("chat:delta", { thread_id: 1001, text: "a", document: null });
    });
    await harness.flush();
    afterStart.chatDelta1UpdateCommits = counts.updates;
    reset(counts);

    act(() => {
      box.events!.emit("chat:delta", { thread_id: 1001, text: "ab", document: { v: 1, blocks: [] } });
    });
    await harness.flush();
    afterStart.chatDelta2UpdateCommits = counts.updates;
    reset(counts);

    act(() => {
      box.events!.emit("chat:delta", { thread_id: 1001, text: "abc", document: { v: 1, blocks: [] } });
    });
    await harness.flush();
    afterStart.chatDelta3UpdateCommits = counts.updates;
    reset(counts);

    act(() => {
      box.events!.emit("chat:done", 1001);
    });
    await harness.flush();
    afterStart.chatDoneUpdateCommits = counts.updates;
    return afterStart;
  }

  it("reports per-flush numeric counts and StrictMode extra effect subscribes", async () => {
    const normal = await run((node) => node);
    harness.cleanup();
    expect(box.events?.activeCount()).toBe(0);

    installStub();
    const strict = await run((node) => <StrictMode>{node}</StrictMode>);
    const extra = {
      strictExtraEffectSubscribes: strict.eventSubscribeCount - normal.eventSubscribeCount,
      strictExtraProfilerMountCommits:
        strict.profilerMountCommitsAfterStartFlush - normal.profilerMountCommitsAfterStartFlush,
    };
    const report = { normal, strict, extra };
    console.info(JSON.stringify(report));

    for (const row of [normal, strict]) {
      for (const value of Object.values(row)) {
        expect(Number.isInteger(value)).toBe(true);
      }
      expect(row.profilerMountCommitsAfterStartFlush).toBeGreaterThan(0);
      expect(row.eventSubscribeCount).toBeGreaterThan(0);
      expect(row.chatDelta1UpdateCommits).toBeGreaterThan(0);
      expect(row.chatDelta2UpdateCommits).toBeGreaterThan(0);
      expect(row.chatDelta3UpdateCommits).toBeGreaterThan(0);
    }
    expect(Number.isInteger(extra.strictExtraEffectSubscribes)).toBe(true);
    expect(Number.isInteger(extra.strictExtraProfilerMountCommits)).toBe(true);
    expect(strict.eventSubscribeCount).not.toBe(normal.eventSubscribeCount);
  });
});
