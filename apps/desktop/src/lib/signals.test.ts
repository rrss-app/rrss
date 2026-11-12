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
  article_id: articleId, opened_at: 1, active_ms: 2, idle_ms: 0, read_chars: 3, skimmed_chars: 0, total_chars: 3,
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

  it("uses one session id and variant for every exposure in a run", async () => {
    const seen: Array<{ session_id: string; variant: string }> = [];
    stub.handle("recordExposures", (rows) => { seen.push(...rows); return rows.map(() => 1); });
    signals.trackExposure(1, "focus", 0);
    await signals.flushSignals();
    signals.trackExposure(2, "focus", 1);
    await signals.flushSignals();
    expect(new Set(seen.map((row) => row.session_id)).size).toBe(1);
    expect(seen.every((row) => row.variant === "baseline")).toBe(true);
  });

  it("records spot checks separately and keeps the first exposure variant", async () => {
    const seen: Array<{ article_id: number; variant: string }> = [];
    stub.handle("recordExposures", (rows) => { seen.push(...rows); return rows.map(() => 1); });
    signals.trackExposure(1, "focus", 0, 100);
    signals.trackExposure(2, "focus", 1, 100, true);
    signals.trackExposure(2, "focus", 1, 200);
    await signals.flushSignals();
    expect(seen.map(({ article_id, variant }) => ({ article_id, variant }))).toEqual([
      { article_id: 1, variant: "baseline" },
      { article_id: 2, variant: "spot_check" },
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
