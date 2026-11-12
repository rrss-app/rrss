import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../lib/api";
import { waitForRefresh } from "./refreshOperation";
import { deferred } from "../../test/deferred";
import type { SchedulerSnapshot } from "../../bindings";

const listeners = vi.hoisted(() => new Set<(snapshot: SchedulerSnapshot) => void>());
vi.mock("../../lib/api", async importOriginal => ({
  ...await importOriginal<typeof import("../../lib/api")>(),
  onSchedulerUpdated: async (listener: (snapshot: SchedulerSnapshot) => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
}));

const snapshot = (done: boolean, version = 1): SchedulerSnapshot => ({
  run_id: "run", version, stopped: false, background: [], background_requested: 0, background_completed: 0, feed_pause: null,
  operations: [{ id: 1, target: 2, completed: done ? 2 : 0, inserted: done ? 3 : 0,
    failed: 0, failed_feeds: [], deleted: 0, skipped: 0, done, error: null }],
});

afterEach(() => { listeners.clear(); vi.restoreAllMocks(); vi.useRealTimers(); });

it("reconciles completion before acknowledgement and cleans its listener", async () => {
  const ack = deferred<{ run_id: string; operation_id: number }>();
  vi.spyOn(api, "schedulerStatus").mockResolvedValue(snapshot(true, 2));
  const result = waitForRefresh(async () => {
    for (const listener of listeners) listener(snapshot(true, 2));
    ack.resolve({ run_id: "run", operation_id: 1 });
    return ack.promise;
  }, new AbortController().signal);
  expect((await result)?.inserted).toBe(3);
  expect(listeners.size).toBe(0);
});

it("recovers a lost completion event and ignores events from another run", async () => {
  vi.useFakeTimers();
  const read = vi.spyOn(api, "schedulerStatus").mockResolvedValue(snapshot(false));
  const result = waitForRefresh(async () => ({ run_id: "run", operation_id: 1 }), new AbortController().signal);
  await vi.advanceTimersByTimeAsync(0);
  for (const listener of listeners) listener({ ...snapshot(true, 100), run_id: "old" });
  read.mockResolvedValue(snapshot(true, 2));
  await vi.advanceTimersByTimeAsync(1000);
  expect((await result)?.completed).toBe(2);
  expect(listeners.size).toBe(0);
});

it("unmount cancels a pending query without applying its late result", async () => {
  const response = deferred<SchedulerSnapshot>();
  vi.spyOn(api, "schedulerStatus").mockReturnValue(response.promise);
  const controller = new AbortController();
  const result = waitForRefresh(async () => ({ run_id: "run", operation_id: 1 }), controller.signal);
  const rejected = expect(result).rejects.toMatchObject({ name: "AbortError" });
  controller.abort();
  response.resolve(snapshot(true));
  await rejected;
  expect(listeners.size).toBe(0);
});
