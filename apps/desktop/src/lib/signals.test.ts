import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  const api = await import("../test/api");
  return { ...actual, api: api.createApiProxy(() => box.stub) };
});

import type { ReadSessionRecord } from "./signals";
let signals: typeof import("./signals");

const session = (articleId: number): ReadSessionRecord => ({
  article_id: articleId, opened_at: 300, active_ms: 2, idle_ms: 0, read_chars: 3, skimmed_chars: 0, total_chars: 3,
  max_block: 1, reached_end: true, translated: false, ended_by: "back", via: "focus",
});

describe("signals", () => {
  let stub: ApiStub;
  beforeEach(async () => {
    vi.resetModules();
    signals = await import("./signals");
    stub = createApiStub();
    box.stub = stub;
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it("does not write before start", async () => {
    vi.useFakeTimers();
    signals.trackEvent(1, "open");
    await vi.advanceTimersByTimeAsync(60_000);
    stub.assertNoUnexpectedCalls();
    expect(stub.calls).toHaveLength(0);
  });

  it("writes exposures first and links events and sessions to their exposure", async () => {
    const order: string[] = [];
    stub.handle("recordExposures", (rows) => { order.push("exposures"); return rows.map((row) => row.article_id * 10); });
    let events: unknown[] = [];
    let sessions: unknown[] = [];
    stub.handle("recordArticleEvents", (rows) => { order.push("events"); events = rows; return rows.length; });
    stub.handle("recordReadSessions", (rows) => { order.push("sessions"); sessions = rows; return rows.length; });
    signals.trackExposure(1, "focus", 0, 100);
    signals.trackExposure(1, "focus", 5, 200);
    signals.trackEvent(1, "open", 300);
    signals.trackEvent(2, "star", 400);
    signals.trackReadSession(session(1));
    await signals.flushSignals();
    expect(order).toEqual(["exposures", "events", "sessions"]);
    expect(stub.callCount("recordExposures")).toBe(1);
    expect(events).toEqual([
      { article_id: 1, kind: "open", at: 300, exposure_id: 10 },
      { article_id: 2, kind: "star", at: 400, exposure_id: null },
    ]);
    expect(sessions).toEqual([{ ...session(1), exposure_id: 10 }]);
  });

  it("rotates UTC sessions across seven resident days and deduplicates across surfaces within each day", async () => {
    vi.useFakeTimers({ now: Date.UTC(2024, 0, 1) });
    const seen: Array<{ article_id: number; session_id: string; variant: string }> = [];
    stub.handle("recordExposures", (rows) => { seen.push(...rows); return rows.map(() => 1); });
    for (let day = 0; day < 7; day++) {
      const id = signals.currentExposureSession();
      signals.trackExposure(1, "focus", 0);
      signals.trackExposure(1, "search", 2);
      signals.trackExposure(day + 2, "focus", 1);
      expect(signals.currentExposureSession(Date.now() + 60_000)).toBe(id);
      await signals.flushSignals();
      await vi.advanceTimersByTimeAsync(86_400_000);
    }
    expect(seen.filter(row => row.article_id === 1)).toHaveLength(7);
    expect(seen).toHaveLength(14);
    expect(new Set(seen.map((row) => row.session_id)).size).toBe(7);
    expect(seen.every((row) => row.variant === "baseline")).toBe(true);
  });

  it("binds queued exposures to their session before a batch spanning midnight is written", async () => {
    vi.useFakeTimers({ now: Date.UTC(2024, 0, 1, 23, 59, 59) });
    let ids: string[] = [];
    stub.handle("recordExposures", rows => { ids = rows.map(row => row.session_id); return [11, 22]; });
    const first = signals.currentExposureSession();
    signals.trackExposure(1, "focus", 0);
    vi.setSystemTime(Date.now() + 1000);
    const second = signals.currentExposureSession();
    signals.trackExposure(1, "focus", 0);
    await signals.flushSignals();
    expect(first).not.toBe(second);
    expect(ids).toEqual([first, second]);
  });

  it("keeps late exposure responses and pending actions attached to their original session", async () => {
    vi.useFakeTimers({ now: Date.UTC(2024, 0, 1) });
    signals.currentExposureSession();
    const pending = stub.deferNext("recordExposures");
    const links: Array<number | null> = [];
    const readLinks: Array<number | null> = [];
    const openedAt = Date.now();
    stub.handle("recordExposures", () => [22]);
    stub.handle("recordArticleEvents", rows => { links.push(...rows.map(row => row.exposure_id)); return rows.length; });
    stub.handle("recordReadSessions", rows => { readLinks.push(...rows.map(row => row.exposure_id)); return rows.length; });
    signals.trackExposure(1, "focus", 0);
    signals.trackEvent(1, "open");
    signals.trackReadSession({ ...session(1), opened_at: openedAt });
    const oldFlush = signals.flushSignals();
    vi.setSystemTime(Date.now() + 86_400_000);
    signals.currentExposureSession();
    signals.trackExposure(1, "focus", 0);
    signals.trackEvent(1, "star");
    signals.trackReadSession({ ...session(1), opened_at: openedAt });
    signals.trackReadSession({ ...session(1), opened_at: Date.now() });
    pending.resolve([11]);
    await oldFlush;
    // The old response must not become today's link, even before today's exposure is written.
    signals.trackEvent(1, "up");
    await signals.flushSignals();
    expect(links).toEqual([11, 22, 22]);
    expect(readLinks).toEqual([11, 11, 22]);
  });

  it("links cross-day reading to the latest exposure before opening, including pending writes", async () => {
    vi.useFakeTimers({ now: Date.UTC(2024, 0, 1) });
    signals.currentExposureSession();
    let nextId = 10;
    const links: Array<number | null> = [];
    stub.handle("recordExposures", rows => rows.map(() => ++nextId));
    stub.handle("recordReadSessions", rows => { links.push(...rows.map(row => row.exposure_id)); return rows.length; });
    signals.trackExposure(1, "focus", 0);
    await signals.flushSignals();
    vi.setSystemTime(Date.now() + 86_400_000);
    signals.currentExposureSession();
    const openedAt = Date.now();
    signals.trackReadSession({ ...session(1), opened_at: openedAt });
    await signals.flushSignals();
    vi.setSystemTime(openedAt + 1000);
    signals.trackExposure(1, "focus", 0);
    signals.trackExposure(2, "focus", 1);
    // Finishing later still selects yesterday's exposure; an unseen article gets no future link.
    signals.trackReadSession({ ...session(1), opened_at: openedAt });
    signals.trackReadSession({ ...session(2), opened_at: openedAt });
    signals.trackReadSession({ ...session(1), opened_at: Date.now() });
    await signals.flushSignals();
    expect(links).toEqual([11, 11, null, 12]);
  });

  it("records exploration and spot checks separately and keeps the first exposure variant", async () => {
    const seen: Array<{ article_id: number; variant: string }> = [];
    stub.handle("recordExposures", (rows) => { seen.push(...rows); return rows.map(() => 1); });
    signals.trackExposure(1, "focus", 0, 100);
    signals.trackExposure(2, "focus", 1, 100, true);
    signals.trackExposure(2, "focus", 1, 200);
    signals.trackExposure(3, "focus", 4, 100, false, true);
    signals.trackExposure(3, "focus", 4, 200);
    await signals.flushSignals();
    expect(seen.map(({ article_id, variant }) => ({ article_id, variant }))).toEqual([
      { article_id: 1, variant: "baseline" },
      { article_id: 2, variant: "spot_check" },
      { article_id: 3, variant: "explore" },
    ]);
  });

  it("flushes on a timer after start and drops a failed batch without throwing", async () => {
    vi.useFakeTimers();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    stub.handle("recordArticleEvents", () => { throw new Error("offline"); });
    const stop = signals.startSignals();
    signals.trackEvent(1, "open");
    await vi.advanceTimersByTimeAsync(5_000);
    expect(stub.callCount("recordArticleEvents")).toBe(1);
    expect(warn).toHaveBeenCalledTimes(1);
    signals.trackEvent(2, "open");
    await vi.advanceTimersByTimeAsync(5_000);
    expect(stub.callCount("recordArticleEvents")).toBe(2);
    expect(warn).toHaveBeenCalledTimes(1);
    stop();
    warn.mockRestore();
  });
});
