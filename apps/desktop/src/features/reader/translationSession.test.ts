import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TranslationEvent, TranslationHint, TranslationSnapshot } from "../../bindings";
import { createApiStub, type ApiStub } from "../../test/api";
import { createEventStub, type EventStub } from "../../test/events";
import { readerView, translationReply, translationSnapshot } from "../../test/fixtures/reader";
import { createTranslationSession, type TranslationSessionView } from "./translationSession";
import { parseDocument } from "../../lib/blockIr";
import { applyTranslationEvent, restoreTranslation, translationNotes } from "./translationState";
import { VIEWPORT_DEBOUNCE_MS, VIEWPORT_MAX_WAIT_MS } from "./translationViewport";
import { translationAction } from "./TranslationStatus";

const box = vi.hoisted(() => ({ api: null as ApiStub | null, events: null as EventStub | null }));
vi.mock("../../lib/api", async () => {
  const { createApiProxy } = await import("../../test/api");
  const { eventListenerExports } = await import("../../test/events");
  return { api: createApiProxy(() => box.api), ...eventListenerExports(() => box.events) };
});
const reader = readerView(1);
const initial = translationSnapshot(reader);
const done: TranslationSnapshot = { ...initial, seq: 2, units: ["done", "done"], phase: "completed", save: "unsaved", title: "Title",
  blocks: [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: "Translated paragraph" }] } }] };
function event(snapshot: TranslationSnapshot = done, extra: Partial<TranslationEvent> = {}): TranslationEvent {
  return { ...snapshot, total: snapshot.units.length, done: snapshot.units.filter(unit => unit === "done").length,
    failed: snapshot.units.filter(unit => typeof unit === "object").length, kind: "delta", units: snapshot.units.map((unit, i) => [i, unit]), ...extra };
}
const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };

describe("progressive reader session", () => {
  let controller: ReturnType<typeof createTranslationSession> | undefined;
  let state: TranslationSessionView;
  const sourceChanged = vi.fn();
  beforeEach(() => {
    vi.useFakeTimers(); sourceChanged.mockReset();
    box.api = createApiStub(); box.events = createEventStub();
    box.api.handle("translateSnapshot", () => translationReply(reader));
    box.api.handle("translateAttach", () => {}); box.api.handle("translateDetach", () => {});
    box.api.handle("translateHint", () => {});
  });
  afterEach(async () => {
    controller?.dispose(); await flush(); box.api!.assertNoUnexpectedCalls(); vi.useRealTimers();
  });
  async function setup() {
    controller = createTranslationSession(reader, value => { state = value; }, sourceChanged); await flush();
  }
  async function start(snapshot = initial) {
    box.api!.handle("translate", () => ({ ...translationReply(reader, snapshot), kind: "started" as const }));
    await controller!.start(); await flush();
  }
  it("keeps typed proxy failures visible when starting translation", async () => {
    await setup();
    box.api!.handle("translate", () => { throw { kind: "configuration", message: "", error: { code: "proxy_url_invalid", detail: "diagnostic canary" } }; });
    await controller!.start(); await flush();
    expect(state!.failure).toBe("configuration");
    expect(state!.issue).toBe(true);
    expect(state!.note).not.toContain("canary");
    expect(state!.note.length).toBeGreaterThan(0);
  });
  it("starts with the latest viewport and bounds continuous hints without queueing passed regions", async () => {
    const hints: TranslationHint[] = [];
    box.api!.handle("translateHint", hint => { hints.push(hint); });
    await setup();
    controller!.setViewport({ visible: [2], nearby: [1, 3] });
    const generate = vi.fn(() => ({ ...translationReply(reader, initial), kind: "started" as const }));
    box.api!.handle("translate", generate);
    await controller!.start(); await flush();
    expect(generate.mock.calls[0]).toEqual([reader.id, "default", expect.objectContaining({ source_hash: reader.source_hash, intent: 1, viewport: { visible: [2], nearby: [1, 3] } })]);
    // 启动已带初始视口附着，不再追加一次附着请求。
    expect(box.api!.callCount("translateAttach")).toBe(0);
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(hints).toHaveLength(0);
    for (let i = 0; i < 6; i++) {
      controller!.setViewport({ visible: [50 + i], nearby: [] });
      await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS / 6);
    }
    expect(hints).toHaveLength(1); expect(hints[0].viewport.visible).toEqual([55]);
    const pending = box.api!.deferNext("translateHint");
    controller!.setViewport({ visible: [200], nearby: [] });
    await vi.advanceTimersByTimeAsync(VIEWPORT_DEBOUNCE_MS);
    for (const id of [20, 100, 80]) {
      controller!.setViewport({ visible: [id], nearby: [] });
      await vi.advanceTimersByTimeAsync(VIEWPORT_DEBOUNCE_MS);
    }
    expect(box.api!.callCount("translateHint")).toBe(2);
    pending.resolve(); await flush();
    expect(hints).toHaveLength(2); expect(hints[1].viewport.visible).toEqual([80]);
    expect(hints[1].intent).toBeGreaterThan(hints[0].intent);
    controller!.setViewport({ visible: [80, 80], nearby: [80] });
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(box.api!.callCount("translateHint")).toBe(3);
    controller!.setViewport({ visible: [30], nearby: [] });
    controller!.dispose();
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(box.api!.callCount("translateHint")).toBe(3);
  });
  it("waits for attachment, sends current geometry on recovery and stops hints at terminal", async () => {
    box.api!.handle("translateSnapshot", () => translationReply(reader, initial));
    const attach = box.api!.deferNext("translateAttach");
    await setup();
    const hints: TranslationHint[] = [];
    box.api!.handle("translateHint", hint => { hints.push(hint); });
    controller!.setViewport({ visible: [4], nearby: [3, 5] });
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(hints).toHaveLength(0);
    attach.resolve(); await flush();
    await vi.advanceTimersByTimeAsync(VIEWPORT_DEBOUNCE_MS);
    expect(hints).toHaveLength(1);
    expect(hints[0].identity).toEqual(initial.identity);
    expect(box.api!.callCount("translate")).toBe(0);
    const snapshot = box.api!.deferNext("translateSnapshot");
    const recovery = controller!.recover(); await flush();
    controller!.setViewport({ visible: [6], nearby: [] });
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(hints).toHaveLength(1);
    snapshot.resolve(translationReply(reader, initial)); await recovery;
    await vi.advanceTimersByTimeAsync(VIEWPORT_DEBOUNCE_MS);
    expect(hints).toHaveLength(2);
    expect(hints[1].viewport.visible).toEqual([6]);
    controller!.setViewport({ visible: [8], nearby: [] });
    box.events!.emit("translate:delta", event(done, { save: "saved" }));
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(hints).toHaveLength(2);
  });
  it("retries a failed attachment only during recovery so viewport hints can resume", async () => {
    box.api!.handle("translateSnapshot", () => translationReply(reader, initial));
    const attach = box.api!.deferNext("translateAttach");
    await setup();
    controller!.setViewport({ visible: [0], nearby: [] });
    attach.reject(new Error("temporary disconnect")); await flush();
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(box.api!.callCount("translateHint")).toBe(0);
    await controller!.recover(); await flush();
    await vi.advanceTimersByTimeAsync(VIEWPORT_DEBOUNCE_MS);
    expect(box.api!.callCount("translateAttach")).toBe(2);
    expect(box.api!.callCount("translateHint")).toBe(1);
    expect(box.api!.callCount("translate")).toBe(0);
  });
  it("keeps a late initial viewport attached while the reader stays open, releasing it on close", async () => {
    await setup();
    controller!.setViewport({ visible: [0], nearby: [] });
    const response = box.api!.deferNext("translate"), detach = vi.fn(), hints: TranslationHint[] = [];
    box.api!.handle("translateDetach", detach);
    box.api!.handle("translateHint", hint => { hints.push(hint); });
    const request = controller!.start(); await flush();
    await vi.advanceTimersByTimeAsync(15_000); await request;
    expect(state.disconnected).toBe(true);
    response.resolve(translationReply(reader, initial)); await flush();
    expect(detach).not.toHaveBeenCalled();
    box.api!.handle("translateSnapshot", () => translationReply(reader, initial));
    await controller!.recover(); await flush();
    controller!.setViewport({ visible: [1], nearby: [] });
    await vi.advanceTimersByTimeAsync(VIEWPORT_MAX_WAIT_MS);
    expect(hints.length).toBeGreaterThan(0);
    expect(hints.at(-1)).toMatchObject({ identity: initial.identity, viewport: { visible: [1] } });
    controller!.dispose(); await flush();
    expect(detach).toHaveBeenCalledWith(initial.identity.generation, expect.any(Number));
  });
  it("releases an initial viewport whose start reply arrives after timeout and close", async () => {
    await setup();
    controller!.setViewport({ visible: [0], nearby: [] });
    const response = box.api!.deferNext("translate"), detach = vi.fn();
    box.api!.handle("translateDetach", detach);
    const request = controller!.start(); await flush();
    await vi.advanceTimersByTimeAsync(15_000); await request;
    expect(state.disconnected).toBe(true);
    controller!.dispose();
    response.resolve(translationReply(reader, initial)); await flush();
    expect(detach).toHaveBeenCalledWith(initial.identity.generation, expect.any(Number));
    expect(box.api!.callCount("translateHint")).toBe(0);
  });
  it("restores on open without generating, and first batch is readable before persistence", async () => {
    await setup(); expect(box.api!.callCount("translate")).toBe(0);
    await start();
    box.events!.emit("translate:delta", event());
    expect(state.data?.ready).toBe(true); expect(state.busy).toBe(true);
    const block = state.data!.document.blocks[0];
    box.events!.emit("translate:delta", event());
    expect(state.data!.document.blocks[0]).toBe(block);
    box.events!.emit("translate:done", event(done, { kind: "done", seq: 3, save: "saved", blocks: [], units: [], title: null }));
    expect(state.busy).toBe(false); expect(state.autoBilingual).toBe(true);
  });
  it("buffers events arriving before the start response and does not start twice", async () => {
    await setup();
    const response = box.api!.deferNext("translate");
    const request = controller!.start(); await flush();
    await controller!.start();
    box.events!.emit("translate:delta", event());
    response.resolve({ ...translationReply(reader, initial), kind: "started" as const }); await request;
    expect(box.api!.callCount("translate")).toBe(1);
    expect(state.data?.snapshot.seq).toBe(2); expect(state.data?.ready).toBe(true);
  });
  it("reconciles a sequence gap once and rejects old or different-generation events", async () => {
    await setup(); await start();
    const snapshot = { ...done, seq: 5, save: "saved" as const };
    box.api!.handle("translateSnapshot", () => translationReply(reader, snapshot));
    box.events!.emit("translate:done", event(snapshot, { kind: "done" })); await flush();
    expect(box.api!.callCount("translateSnapshot")).toBe(2);
    expect(state.data?.snapshot.seq).toBe(5); expect(state.busy).toBe(false);
    box.events!.emit("translate:delta", event());
    box.events!.emit("translate:delta", event(snapshot, { seq: 6, identity: { ...snapshot.identity, generation: 99 } }));
    expect(state.data?.snapshot.seq).toBe(5);
  });
  it("bounds handshake buffering, recovering overflow with a read-only snapshot", async () => {
    await setup(); const response = box.api!.deferNext("translate");
    const request = controller!.start(); await flush();
    for (let seq = 2; seq < 140; seq++) box.events!.emit("translate:delta", event(done, { seq }));
    box.api!.handle("translateSnapshot", () => translationReply(reader, { ...done, seq: 140, save: "saved" }));
    response.resolve({ ...translationReply(reader, initial), kind: "started" as const }); await request; await flush();
    expect(state.data?.snapshot.seq).toBe(140); expect(box.api!.callCount("translate")).toBe(1);
    expect(box.api!.callCount("translateSnapshot")).toBe(2);
  });
  it("missed terminal event is reconciled after the backend interval; failed recovery stops polling", async () => {
    await setup(); await start();
    box.api!.handle("translateSnapshot", () => { throw new Error("disconnected"); });
    await vi.advanceTimersByTimeAsync(45_000); await flush();
    expect(state.disconnected).toBe(true); expect(state.busy).toBe(false);
    const calls = box.api!.callCount("translateSnapshot");
    await vi.advanceTimersByTimeAsync(180_000); expect(box.api!.callCount("translateSnapshot")).toBe(calls);
    box.api!.handle("translateSnapshot", () => translationReply(reader, { ...done, save: "saved" }));
    await controller!.recover(); expect(state.data?.ready).toBe(true); expect(state.autoBilingual).toBe(true);
    expect(box.api!.callCount("translate")).toBe(1);
  });
  it("registration failure blocks generation and removes the partial listeners", async () => {
    const gate = box.events!.deferListen("translate:delta");
    controller = createTranslationSession(reader, value => { state = value; }, sourceChanged);
    await controller.start(); expect(box.api!.callCount("translate")).toBe(0);
    gate.reject(new Error("no event channel")); await flush();
    expect(state.disconnected).toBe(true); expect(box.events!.activeCount()).toBe(0);
    expect(box.api!.callCount("translateSnapshot")).toBe(0);
  });
  it("a listener resolving after timeout is cleaned up without starting a request", async () => {
    const gate = box.events!.deferListen("translate:delta");
    controller = createTranslationSession(reader, value => { state = value; }, sourceChanged);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(state.disconnected).toBe(true);
    gate.resolve(); await flush();
    expect(box.events!.activeCount()).toBe(0);
    expect(box.api!.callCount("translateSnapshot")).toBe(0);
    await controller.recover();
    expect(state.disconnected).toBe(false);
    expect(box.events!.activeCount()).toBe(4);
  });
  it("an older recovery cannot discard newer results, and expired retries offer regeneration", async () => {
    await setup(); await start();
    box.events!.emit("translate:delta", event({ ...done, save: "saved" }));
    box.api!.handle("translateSnapshot", () => translationReply(reader, initial));
    await controller!.recover();
    expect(state.disconnected).toBe(true); expect(state.data?.snapshot.seq).toBe(2);
    box.api!.handle("translate", () => { throw { kind: "retry_unavailable", message: "记录已过期" }; });
    await controller!.start("retry_failed");
    expect(translationAction(state).kind).toBe("regenerate");
  });
  it("late responses and events after closing cannot change the reader or attach it", async () => {
    await setup(); const response = box.api!.deferNext("translate");
    const request = controller!.start(); await flush(); const before = state;
    controller!.dispose(); response.resolve(translationReply(reader, done)); await request;
    box.events!.emit("translate:delta", event());
    expect(state).toBe(before); expect(box.api!.callCount("translateAttach")).toBe(0);
    expect(box.events!.activeCount()).toBe(0);
  });
  it("save-only retry keeps the translated content and never calls generation", async () => {
    const failed = { ...done, save: "save_failed" as const, error: "storage" as const };
    box.api!.handle("translateSnapshot", () => translationReply(reader, failed));
    box.api!.handle("translateRetrySave", () => translationReply(reader, { ...done, save: "saved", error: null }));
    await setup(); expect(state.autoBilingual).toBe(false);
    const content = state.data!.document.blocks;
    await controller!.retrySave();
    expect(state.data!.document.blocks).toEqual(content); expect(state.busy).toBe(false);
    expect(box.api!.callCount("translate")).toBe(0); expect(state.data?.snapshot.save).toBe("saved");
  });
  it("source replacement is applied as a fresh reader without merging old blocks", async () => {
    await setup(); await start();
    box.api!.handle("translateSnapshot", () => translationReply({ ...reader, source_hash: "replacement" }));
    await controller!.recover(); expect(sourceChanged).toHaveBeenCalledWith(expect.objectContaining({ source_hash: "replacement" }));
    expect(state.data?.snapshot.identity.source_hash).toBe(reader.source_hash);
  });
  it("reopening an all-failed regeneration keeps the valid cache, but a new config cannot reuse it", async () => {
    const cachedReader = readerView(1, { translation_ready: true, translated_title: "Old title", translated_document: { v: 1, blocks: [done.blocks[0].ir] } });
    const failed: TranslationSnapshot = { ...initial, phase: "failed", units: [{ failed: "network" }, { failed: "network" }], error: "network" };
    box.api!.handle("translateSnapshot", () => translationReply(cachedReader, failed));
    controller = createTranslationSession(cachedReader, value => { state = value; }, sourceChanged); await flush();
    expect(state.data?.ready).toBe(true); expect(state.data?.title).toBe("Old title");
    expect(state.autoBilingual).toBe(false);
    box.api!.handle("translateSnapshot", () => translationReply(reader, { ...initial, identity: { ...initial.identity, generation: 11, config_hash: "new-config" } }));
    await controller.recover();
    expect(state.data?.ready).toBe(false); expect(state.data?.title).toBe("");
  });
  it("a saved terminal event clears the storage failure while preserving content", () => {
    const current = restoreTranslation(reader, { ...done, save: "save_failed", error: "storage" });
    const saved = applyTranslationEvent(current, event(done, { kind: "done", seq: 3, save: "saved", blocks: [], units: [], title: null, error: null }));
    expect(saved.snapshot.error).toBeNull(); expect(saved.document.blocks[0]).toBe(current.document.blocks[0]);
  });
  it("failed units say whether they show the original or a retained translation, and all-failed retries without regeneration", () => {
    const partial: TranslationSnapshot = { ...initial, identity: { ...initial.identity, generation: null }, phase: "partial", save: "saved",
      units: ["done", { failed: "network" }], title: "Title" };
    const reopen = (translated: unknown) => {
      const cachedReader = readerView(1, { translation_ready: true, translated_title: "Title", translated_document: translated as never });
      return translationNotes(restoreTranslation(cachedReader, partial), parseDocument(cachedReader.document), "bilingual", false).get(0);
    };
    // 部分缓存里失败处存的是原文副本：内容相同但对象不同。
    expect(reopen(structuredClone(reader.document))).toBe("本处未译出 · 已保留原文");
    expect(reopen({ v: 1, blocks: [done.blocks[0].ir] })).toBe("本次未译出 · 保留已有译文");
    const failed = restoreTranslation(reader, { ...initial, phase: "failed", units: [{ failed: "network" }, { failed: "network" }], error: "network" });
    expect(translationAction({ data: failed, autoBilingual: false, busy: false, disconnected: false, issue: false, failure: null, note: "" }).kind).toBe("start");
  });
  it("debug mode appends the failure kind after an untranslated block note", () => {
    const partial: TranslationSnapshot = { ...initial, identity: { ...initial.identity, generation: null }, phase: "partial", save: "saved",
      units: ["done", { failed: "invalid_output" }], title: "Title" };
    const cachedReader = readerView(1, { translation_ready: true, translated_title: "Title", translated_document: structuredClone(reader.document) as never });
    const notes = (debug: boolean) => translationNotes(restoreTranslation(cachedReader, partial), parseDocument(cachedReader.document), "bilingual", debug).get(0);
    expect(notes(true)).toBe("本处未译出 · 已保留原文（invalid_output）");
    expect(notes(false)).toBe("本处未译出 · 已保留原文");
  });
  it("bad patches, unmapped units and count mismatches require recovery", () => {
    const current = restoreTranslation(reader, initial);
    expect(() => applyTranslationEvent(current, event(done, { blocks: [{ index: 9, ir: { t: "hr" } }] }))).toThrow();
    expect(() => applyTranslationEvent(current, event(done, { done: 9 }))).toThrow();
    expect(() => restoreTranslation(reader, { ...done, unit_map: [null, 9] })).toThrow();
    expect(() => restoreTranslation(reader, { ...done, identity: { ...done.identity, source_hash: "wrong" } })).toThrow();
  });
});
