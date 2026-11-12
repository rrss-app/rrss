import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useConversationRuntime } from "../features/chat";
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

function Probe({ onError }: { onError: (message: string) => void }) {
  useConversationRuntime({ onError, onDone: () => {} });
  return <span>conversations</span>;
}

describe("conversation runtime delayed listen cleanup", () => {
  const harness = createReactHarness();

  beforeEach(() => {
    box.stub = createApiStub();
    box.events = createEventStub();
  });

  afterEach(() => {
    harness.cleanup();
    expect(box.events?.activeCount()).toBe(0);
    box.stub?.assertNoUnexpectedCalls();
  });

  it("unsubscribes when listen resolves after unmount", async () => {
    const names = ["chat:start", "chat:delta", "chat:done", "chat:error", "chat:status", "chat:confirm", "chat:context"] as const;
    const gates = names.map((name) => box.events!.deferListen(name));
    const { unmount } = harness.mount(<Probe onError={() => {}} />);
    await harness.flush();
    expect(box.events!.subscribeCount).toBe(names.length);
    expect(box.events!.activeCount()).toBe(0);
    unmount();
    gates.forEach((gate) => gate.resolve());
    await harness.flush();
    expect(box.events!.activeCount()).toBe(0);
    expect(box.events!.unsubscribeCount).toBe(names.length);
  });

  it("does not report listen failure after unmount", async () => {
    const onError = vi.fn();
    const start = box.events!.deferListen("chat:start");
    const delta = box.events!.deferListen("chat:delta");
    const done = box.events!.deferListen("chat:done");
    const error = box.events!.deferListen("chat:error");
    const { unmount } = harness.mount(<Probe onError={onError} />);
    unmount();
    start.reject(new Error("listen failed"));
    delta.reject(new Error("listen failed"));
    done.reject(new Error("listen failed"));
    error.reject(new Error("listen failed"));
    await harness.flush();
    expect(onError).not.toHaveBeenCalled();
    expect(box.events!.activeCount()).toBe(0);
  });
});
