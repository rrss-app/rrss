import { act, type ComponentProps } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import type { TestClock } from "../test/clock";
import type { UiError } from "../bindings";
import { DiagPane } from "./DiagPane";
import { diagnosticsKeys, diagnosticsQueryOptions, prefetchDiagnostics } from "./diagnosticsQuery";
import { memoryKeys } from "./memoryQuery";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
const harness = createReactHarness();
const page = { vec: "可用", stats: "10 篇", db: "ok", ai: "0", log: null, scheduler: null };
let client: QueryClient;
let clock: TestClock;
function mount(props: Partial<ComponentProps<typeof DiagPane>> = {}) { return harness.mount(<QueryClientProvider client={client}><DiagPane onAsk={() => {}} {...props} /></QueryClientProvider>); }
async function flush() { await harness.flush(); await clock.advance(1); await harness.flush(); }
function button(host: HTMLElement, text: string) { return [...host.querySelectorAll("button")].find((item) => item.textContent === text)!; }

describe("diagnostics queries and maintenance", () => {
  beforeEach(() => {
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    clock = harness.useClock();
    client = new QueryClient();
    box.stub = createApiStub();
    box.stub.handle("diagnostics", () => page);
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    vi.unstubAllGlobals();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("shares an in-flight diagnostics query between preload and mounts", async () => {
    const pending = box.stub!.deferNext("diagnostics");
    const preload = prefetchDiagnostics(client);
    mount(); mount();
    await flush();
    expect(box.stub!.callCount("diagnostics")).toBe(1);
    pending.resolve(page); await preload; await flush();
    expect(client.getQueryData(diagnosticsKeys.page)).toEqual(page);
  });

  it("shows a structured IPC loading failure as text", async () => {
    const error = { code: "legacy_text", params: {}, detail: "fixture diagnostics read failure" } satisfies UiError;
    box.stub!.handle("diagnostics", () => { throw error; });
    const { host } = mount(); await flush();
    expect(client.getQueryState(diagnosticsKeys.page)?.error).toMatchObject({ code: "legacy_text" });
    const alert = host.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain(error.detail);
    expect(alert?.textContent).not.toContain("[object Object]");
  });

  it("isolates write-time reads and guards writes across unmount without losing invalidation", async () => {
    client.setQueryData(diagnosticsKeys.page, page);
    client.setQueryData(memoryKeys.page(""), { memories: [], interests: [], inject_preview: "" });
    const pending = box.stub!.deferNext("embedNow");
    const editState = vi.fn();
    const changed = vi.fn();
    const first = mount({ onEditStateChange: editState, onChanged: changed });
    await flush();
    const process = button(first.host, "处理一批");
    act(() => { process.click(); process.click(); });
    await flush();
    expect(editState).toHaveBeenLastCalledWith(expect.objectContaining({ busy: true }));
    await prefetchDiagnostics(client);
    expect(box.stub!.callCount("diagnostics")).toBe(0);
    const late = box.stub!.deferNext("diagnostics");
    const lateRead = client.fetchQuery({ ...diagnosticsQueryOptions(), staleTime: 0 }).catch(() => undefined);
    first.unmount();
    const callsAtUnmount = editState.mock.calls.length;
    const second = mount(); await flush();
    expect(button(second.host, "处理一批").disabled).toBe(true);
    box.stub!.handle("diagnostics", () => ({ ...page, stats: "处理后统计" }));
    pending.resolve(undefined); await flush();
    late.resolve({ ...page, stats: "过时统计" }); await lateRead; await flush();
    expect(box.stub!.callCount("embedNow")).toBe(1);
    expect(box.stub!.callCount("diagnostics")).toBe(2);
    expect(second.host.textContent).toContain("处理后统计");
    expect(second.host.textContent).not.toContain("过时统计");
    expect(changed).toHaveBeenCalledTimes(1);
    expect(client.getQueryState(memoryKeys.page(""))?.isInvalidated).toBe(true);
    expect(editState.mock.calls).toHaveLength(callsAtUnmount);
    expect(editState).toHaveBeenLastCalledWith(null);
  });

});
