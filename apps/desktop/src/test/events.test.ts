import { describe, expect, it, vi } from "vitest";
import { createEventStub, type EventStub } from "./events";

describe("createEventStub", () => {
  it("notifies every subscriber and keeps delta snapshots as-is", async () => {
    const events = createEventStub();
    const first = vi.fn();
    const second = vi.fn();
    const offFirst = await events.onChatDelta(first);
    const offSecond = await events.onChatDelta(second);
    const snapA = { thread_id: 1, text: "ab", document: null };
    const snapB = { thread_id: 1, text: "abcd", document: null };
    events.emit("chat:delta", snapA);
    events.emit("chat:delta", snapB);
    expect(first.mock.calls.map((call) => call[0])).toEqual([snapA, snapB]);
    expect(second.mock.calls.map((call) => call[0])).toEqual([snapA, snapB]);
    expect(first.mock.calls[1][0].text).toBe("abcd");
    offFirst();
    offSecond();
    expect(events.activeCount("chat:delta")).toBe(0);
    events.emit("chat:delta", { thread_id: 1, text: "ignored", document: null });
    expect(first).toHaveBeenCalledTimes(2);
    expect(second).toHaveBeenCalledTimes(2);
  });

  it("counts subscribe, idempotent unsubscribe, and active listeners", async () => {
    const events = createEventStub();
    const off = await events.onChatDone(() => {});
    expect(events.subscribeCount).toBe(1);
    expect(events.activeCount("chat:done")).toBe(1);
    off();
    off();
    expect(events.unsubscribeCount).toBe(1);
    expect(events.activeCount()).toBe(0);
  });

  it("does not attach a listener when delayed listen rejects", async () => {
    const events = createEventStub();
    const gate = events.deferListen("chat:error");
    const pending = events.onChatError(() => {});
    gate.reject(new Error("listen failed"));
    await expect(pending).rejects.toThrow("listen failed");
    expect(events.subscribeCount).toBe(1);
    expect(events.activeCount()).toBe(0);
  });

  it("unsubscribes if listen resolves after the caller already disposed", async () => {
    const events = createEventStub();
    const gate = events.deferListen("chat:start");
    let disposed = false;
    const pending = events.onChatStart(() => {});
    void pending.then((unsub) => {
      if (disposed) unsub();
    });
    disposed = true;
    gate.resolve();
    await pending;
    expect(events.activeCount()).toBe(0);
    expect(events.unsubscribeCount).toBe(1);
  });

  it("keeps listener maps on separate instances", async () => {
    const a = createEventStub();
    const b = createEventStub();
    const fromA = vi.fn();
    const off = await a.onChatStart(fromA);
    b.emit("chat:start", 9);
    expect(fromA).not.toHaveBeenCalled();
    a.emit("chat:start", 3);
    expect(fromA).toHaveBeenCalledWith(3);
    off();
    a.emit("chat:start", 4);
    expect(fromA).toHaveBeenCalledTimes(1);
  });

  it("delivers chat:error and media:ready payloads and stops after unsubscribe", async () => {
    const events = createEventStub();
    const onError = vi.fn();
    const onMedia = vi.fn();
    const offError = await events.onChatError(onError);
    const offMedia = await events.onMediaReady(onMedia);
    const error = { thread_id: 4, message: { code: "legacy_text" as const, params: {}, detail: "boom" }, kind: "model" };
    const media = { article_id: 8, media: { src: "a.png", href: "rrss://localhost/media/1", width: null, height: null, error: "" } };
    events.emit("chat:error", error);
    events.emit("media:ready", media);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(error);
    expect(onMedia).toHaveBeenCalledTimes(1);
    expect(onMedia).toHaveBeenCalledWith(media);
    offError();
    offMedia();
    events.emit("chat:error", { thread_id: 4, message: { code: "legacy_text", params: {}, detail: "again" }, kind: "model" });
    events.emit("media:ready", media);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onMedia).toHaveBeenCalledTimes(1);
  });
});

function _eventTypeContract(events: EventStub) {
  // @ts-expect-error chat:start payload is a thread id
  events.emit("chat:start", { thread_id: 1 });
  // @ts-expect-error media payload is MediaReadyEvent
  events.emit("media:ready", 1);
}
void _eventTypeContract;
