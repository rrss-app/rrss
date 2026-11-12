import { afterEach, expect, it, vi } from "vitest";
import { App } from "../App";
import type { SchedulerSnapshot } from "../bindings";
import { createApiStub, type ApiStub } from "../test/api";
import { createEventStub, type EventStub } from "../test/events";
import { click, clickText, handleShellReads } from "../test/fixtures/shell";
import { createReactHarness } from "../test/react";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null, events: null as EventStub | null }));
vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  const api = await import("../test/api");
  const events = await import("../test/events");
  return { ...actual, api: api.createApiProxy(() => box.stub), ...events.eventListenerExports(() => box.events) };
});

const harness = createReactHarness();
const win = window as Window & { __TAURI_INTERNALS__?: unknown };
afterEach(() => {
  harness.cleanup();
  vi.restoreAllMocks();
  delete win.__TAURI_INTERNALS__;
  box.stub?.assertNoUnexpectedCalls();
});

it("reports paused refresh in the existing notice and retries after connectivity returns", async () => {
  win.__TAURI_INTERNALS__ = {};
  box.stub = createApiStub();
  box.events = createEventStub();
  handleShellReads(box.stub);
  let available = false;
  let operation = 0;
  let snapshot: SchedulerSnapshot = {
    run_id: "network-run", version: 1, stopped: false, operations: [],
    background: [], background_requested: 0, background_completed: 0,
    feed_pause: "网络不可用",
  };
  box.stub.handle("schedulerStatus", () => snapshot);
  box.stub.handle("refresh", () => {
    const id = ++operation;
    snapshot = {
      ...snapshot, version: snapshot.version + 1, feed_pause: available ? null : snapshot.feed_pause,
      operations: [{ id, target: 3, completed: 3, inserted: available ? 1 : 0,
        failed: 0, failed_feeds: [], deleted: 0, skipped: available ? 0 : 3,
        done: true, error: available ? null : snapshot.feed_pause }],
    };
    return { run_id: snapshot.run_id, operation_id: id };
  });
  const { host } = harness.mount(<App />);
  await harness.flush();
  const refresh = host.querySelector<HTMLButtonElement>('button[aria-label="刷新"]')!;
  click(refresh);
  await harness.flush();
  await harness.flush();
  expect(refresh.disabled).toBe(false);
  expect(snapshot.operations[0].skipped).toBe(3);
  available = true;
  clickText(host, "重试");
  await harness.flush();
  await harness.flush();
  expect(refresh.disabled).toBe(false);
  expect(snapshot.operations[0].skipped).toBe(0);
  expect(snapshot.operations[0].inserted).toBe(1);
  expect(snapshot.feed_pause).toBeNull();
});
