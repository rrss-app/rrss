import { afterEach, beforeEach, expect, it } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { flushUpdates } from "../test/react";
import { createMcpRuntime, inputFromView } from "./mcpRuntime";
import { mcpFixture } from "../test/fixtures/settings";

let stub: ApiStub;
let runtime: ReturnType<typeof createMcpRuntime>;
beforeEach(() => {
  stub = createApiStub();
  runtime = createMcpRuntime(stub.api);
});
afterEach(() => stub.assertNoUnexpectedCalls());

it("keeps the new login pending when a cancelled request finishes late", async () => {
  const old = stub.deferNext("loginMcp");
  const current = stub.deferNext("loginMcp");
  stub.handle("cancelMcpLogin", () => {});
  runtime.startLogin(7);
  await flushUpdates();
  runtime.cancelLogin(7);
  runtime.startLogin(7);
  old.resolve({ ok: false, message: "cancelled request" });
  await flushUpdates();
  expect(runtime.takeLogin(7)).toBeUndefined();
  current.resolve({ ok: true, message: "current request" });
  await flushUpdates();
  expect(runtime.takeLogin(7)).toMatchObject({ ok: true, message: "current request" });
});

it("waits for the old cancellation to finish before launching another login", async () => {
  const old = stub.deferNext("loginMcp");
  const cancelled = stub.deferNext("cancelMcpLogin");
  let nextStarted = false;
  stub.handle("loginMcp", () => {
    nextStarted = true;
    return { ok: true, message: "next request" };
  });
  runtime.startLogin(7);
  await flushUpdates();
  runtime.cancelLogin(7);
  runtime.startLogin(7);
  old.resolve({ ok: true, message: "old request" });
  await flushUpdates();
  expect(nextStarted).toBe(false);
  expect(runtime.takeLogin(7)).toBeUndefined();
  cancelled.resolve();
  await flushUpdates();
  expect(runtime.takeLogin(7)).toMatchObject({ ok: true, message: "next request" });
});

it("does not revive a deleted server when its login completes", async () => {
  const pending = stub.deferNext("loginMcp");
  runtime.startLogin(7);
  await flushUpdates();
  runtime.forgetServer(7);
  pending.resolve({ ok: true, message: "deleted server" });
  await flushUpdates();
  expect(runtime.takeLogin(7)).toBeUndefined();
});

it("does not consume a replacement login when an older enable operation settles", async () => {
  stub.handle("loginMcp", () => ({ ok: true, message: "" }));
  runtime.startLogin(7);
  await flushUpdates();
  const previous = runtime.logins.get().get(7)!;
  runtime.forgetServer(7);
  runtime.startLogin(7);
  await flushUpdates();
  expect(runtime.takeLogin(7, previous)).toBeUndefined();
  expect(runtime.takeLogin(7)).toMatchObject({ state: "done", ok: true });
});

it("keeps only the current probe and removes it when the connection is forgotten", async () => {
  const input = inputFromView(mcpFixture({ id: 7 }));
  const old = stub.deferNext("probeMcp");
  const current = stub.deferNext("probeMcp");
  runtime.probeServer(input);
  runtime.probeServer(input);
  const result = { ok: true, elapsed_ms: 1, tools: [], error: "", hint: "", output: "current" };
  current.resolve(result);
  await flushUpdates();
  old.reject(new Error("old failure"));
  await flushUpdates();
  expect(runtime.probes.get().get(7)?.result).toEqual(result);
  const deleted = stub.deferNext("probeMcp");
  runtime.probeServer(input);
  runtime.forgetServer(7);
  deleted.resolve(result);
  await flushUpdates();
  expect(runtime.probes.get().has(7)).toBe(false);
});

it("retains a typed failure from a rejected probe command", async () => {
  const input = inputFromView(mcpFixture({ id: 7 }));
  stub.handle("probeMcp", () => Promise.reject({ code: "mcp_http_status", params: { status: 503 }, detail: "fixture" }));
  runtime.probeServer(input);
  await flushUpdates();
  expect(runtime.probes.get().get(7)?.result?.failure).toMatchObject({ code: "mcp_http_status", params: { status: 503 } });
});
