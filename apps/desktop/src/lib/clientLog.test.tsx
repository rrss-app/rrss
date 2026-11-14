import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  const api = await import("../test/api");
  return { ...actual, api: api.createApiProxy(() => box.stub) };
});
vi.mock("./preview", () => ({ isTauri: () => true }));

import { startClientLog, reportClientError } from "./clientLog";
import { ErrorBoundary } from "../components/ErrorBoundary";

let stop: () => void;
const react = createReactHarness();
let now = 0;
beforeEach(() => {
  now = 0;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  box.stub = createApiStub();
  stop = startClientLog();
});
afterEach(() => { stop(); react.cleanup(); vi.restoreAllMocks(); });

it("reports a render failure and uncaught events while deduplicating and limiting bursts", async () => {
  const reports: { scope: string; message: string }[] = [];
  box.stub!.handle("logClient", (_level, scope, message) => { reports.push({ scope, message }); });
  vi.spyOn(console, "error").mockImplementation(() => {});
  function Broken(): never { throw new Error("render failure"); }
  react.mount(<ErrorBoundary scope="reader" fallback={() => null}><Broken /></ErrorBoundary>);
  await Promise.resolve();
  expect(reports).toContainEqual({ scope: "reader", message: "render failure" });
  // Let the test own the synthetic event's default handling; production keeps
  // the browser's normal error reporting intact.
  const event = new ErrorEvent("error", { message: "window failure", cancelable: true });
  event.preventDefault();
  window.dispatchEvent(event);
  window.dispatchEvent(event);
  for (let i = 0; i < 30; i++) reportClientError("burst", new Error(`failure ${i}`));
  await Promise.resolve();
  expect(reports.filter(report => report.message === "window failure")).toHaveLength(1);
  expect(reports).toHaveLength(20);
  now = 60_001;
  window.dispatchEvent(event);
  await Promise.resolve();
  expect(reports.filter(report => report.message === "window failure")).toHaveLength(2);
  stop();
  reportClientError("stopped", new Error("after stop"));
  await Promise.resolve();
  expect(reports).toHaveLength(21);
  box.stub!.assertNoUnexpectedCalls();
});

it("does not serialize rejection objects or recurse when its own IPC fails", async () => {
  const reports: string[] = [];
  box.stub!.handle("logClient", (_level, _scope, message) => {
    reports.push(message);
    throw new Error("report failed");
  });
  const event = new Event("unhandledrejection");
  Object.defineProperty(event, "reason", { value: { body: "canary-content", token: "canary-key" } });
  window.dispatchEvent(event);
  reportClientError("host", {
    code: "legacy_text",
    get detail() { throw new Error("canary-content must not be read"); },
  });
  await Promise.resolve();
  await Promise.resolve();
  expect(reports).toEqual(["non-error rejection", "UI error legacy_text"]);
  box.stub!.assertNoUnexpectedCalls();
});
