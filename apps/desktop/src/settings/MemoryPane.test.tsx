import { act, type ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import type { TestClock } from "../test/clock";
import type { MemoryPage } from "../lib/api";
import { FALLBACK_RANKING } from "../components/FocusPane";
import { MemoryPane } from "./MemoryPane";
import { memoryKeys } from "./memoryQuery";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
const harness = createReactHarness();
const props = { ranking: FALLBACK_RANKING, onRanking: () => {}, onAsk: () => {}, onOpenArticle: () => {} };
const memory = { id: 1, kind: "偏好", content: "原始记忆", pinned: false, pending: true, scope: "", strength: "" };
const interest = { id: 1, label: "本地优先", weight: 1, enabled: true, has_embedding: false, hit_count: 0 };
const page = (content = memory.content): MemoryPage => ({ memories: [{ ...memory, content }], interests: [interest], inject_preview: "" });
let client: QueryClient;
let clock: TestClock;
function mount(node: ReactNode = <MemoryPane {...props} />) {
  return harness.mount(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}
async function flush() { await harness.flush(); await clock.advance(1); await harness.flush(); }
function click(host: HTMLElement, text: string) {
  const button = [...host.querySelectorAll("button")].find((item) => item.textContent === text);
  expect(button).toBeTruthy();
  act(() => button!.click());
}
function input(host: HTMLElement, label: string, value: string) {
  const field = host.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[aria-label="${label}"]`)!;
  act(() => {
    const prototype = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
  return field;
}

function tab(host: HTMLElement, name: string) {
  const button = [...host.querySelectorAll<HTMLButtonElement>(".memory-tabs button")].find((item) => item.textContent?.startsWith(name));
  expect(button).toBeTruthy();
  act(() => button!.click());
}
function search(host: HTMLElement, value: string) {
  const field = input(host, "搜索记忆", value);
  act(() => { field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
}

describe("memory settings feedback", () => {
  beforeEach(() => {
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    clock = harness.useClock();
    client = new QueryClient();
    box.stub = createApiStub();
    box.stub.handle("memory", () => page());
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    vi.unstubAllGlobals();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("keeps bulk deletion outside memory rows and still requires confirmation", async () => {
    const onAsk = vi.fn();
    const { host } = mount(<MemoryPane {...props} onAsk={onAsk} />);
    await flush();
    expect([...host.querySelectorAll(".memory-card button")].some((b) => b.textContent === "清空全部记忆")).toBe(false);
    const details = host.querySelector<HTMLDetailsElement>("details")!;
    expect(details.open).toBe(false);
    act(() => { details.open = true; });
    click(host, "清空全部记忆");
    expect(onAsk).toHaveBeenCalledWith("清空记忆", expect.stringContaining("不可恢复"), expect.any(Function));
    expect(box.stub!.callCount("clearMemories")).toBe(0);
  });

  it("isolates submitted searches, empty results and late responses", async () => {
    const read = vi.fn((q = "") => page(q || "全部记忆"));
    box.stub!.handle("memory", read);
    const { host } = mount();
    await flush();
    const old = box.stub!.deferNext("memory");
    search(host, "旧查询");
    await flush();
    search(host, "新查询");
    await flush();
    old.resolve(page("过时结果"));
    await flush();
    expect(host.querySelector(".memory-content")?.textContent).toBe("新查询");
    input(host, "搜索记忆", "尚未提交");
    expect(read).not.toHaveBeenCalledWith("尚未提交");
    expect(client.getQueryData(memoryKeys.page(""))).toEqual(page("全部记忆"));
    search(host, "");
    await flush();
    expect(host.querySelector(".memory-content")?.textContent).toBe("全部记忆");
  });

  it("finishes a committed memory edit even if only the reread fails", async () => {
    const write = vi.fn();
    box.stub!.handle("editMemory", write);
    const { host } = mount();
    await flush();
    click(host, "编辑"); input(host, "记忆内容", "已经保存的内容");
    box.stub!.handle("memory", () => { throw new Error("reread failed"); });
    click(host, "保存"); await flush();
    expect(write).toHaveBeenCalledTimes(1);
    expect(host.querySelector("textarea")).toBeNull();
    expect(host.textContent).toContain("保存记忆已完成");
    expect(host.textContent).toContain("刚才的操作已完成");
    box.stub!.handle("memory", () => page("已经保存的内容"));
    click(host, "重试读取"); await flush();
    expect(host.querySelector(".memory-content")?.textContent).toBe("已经保存的内容");
    expect(write).toHaveBeenCalledTimes(1);
  });

  it("preserves editor drafts on refresh and write failure and confirms replacement", async () => {
    box.stub!.handle("editMemory", () => { throw new Error("locked"); });
    const onAsk = vi.fn();
    const editState = vi.fn();
    const { host } = mount(<MemoryPane {...props} onAsk={onAsk} onEditStateChange={editState} />);
    await flush();
    click(host, "编辑"); input(host, "记忆内容", "我的草稿");
    act(() => client.setQueryData(memoryKeys.page(""), page("后台新值")));
    await flush();
    expect(host.querySelector<HTMLTextAreaElement>("textarea")!.value).toBe("我的草稿");
    click(host, "保存"); await flush();
    expect(host.querySelector<HTMLTextAreaElement>("textarea")!.value).toBe("我的草稿");
    search(host, "另一条");
    expect(onAsk).toHaveBeenCalledWith("放弃记忆修改", expect.any(String), expect.any(Function));
    expect(editState).toHaveBeenLastCalledWith(expect.objectContaining({ dirty: true, busy: false }));
    expect(host.querySelector<HTMLTextAreaElement>("textarea")!.value).toBe("我的草稿");
    act(() => { onAsk.mock.calls[0][2](); });
    await flush();
    expect(host.querySelector("textarea")).toBeNull();
  });

  it("keeps writes exclusive across remount and still notifies focus after unmount", async () => {
    const pending = box.stub!.deferNext("addInterest");
    const changed = vi.fn();
    const editState = vi.fn();
    const first = mount(<MemoryPane {...props} onChanged={changed} onEditStateChange={editState} />);
    await flush();
    tab(first.host, "兴趣"); input(first.host, "新兴趣标签", "新兴趣"); click(first.host, "添加");
    await flush(); first.unmount();
    const callsAtUnmount = editState.mock.calls.length;
    const second = mount();
    await flush();
    expect(second.host.querySelector("fieldset")!.disabled).toBe(true);
    pending.resolve(2); await flush();
    expect(changed).toHaveBeenCalledTimes(1);
    expect(editState.mock.calls).toHaveLength(callsAtUnmount);
    expect(editState).toHaveBeenLastCalledWith(null);
    expect(second.host.querySelector("fieldset")!.disabled).toBe(false);
    expect(box.stub!.callCount("addInterest")).toBe(1);
  });

  it("rereads the list after a rejected conversion instead of keeping a stale pending card", async () => {
    box.stub!.handle("memoryAsInterest", () => { throw new Error("already confirmed elsewhere"); });
    const changed = vi.fn();
    const { host } = mount(<MemoryPane {...props} onChanged={changed} />);
    await flush();
    box.stub!.handle("memory", () => ({ ...page(), memories: [{ ...memory, pending: false }] }));
    click(host, "添加为兴趣"); await flush();
    expect([...host.querySelectorAll("button")].some((b) => b.textContent === "添加为兴趣")).toBe(false);
    expect(changed).not.toHaveBeenCalled();
  });

  it("notifies focus for conversion, enabling and deletion but not confirmation", async () => {
    box.stub!.handle("memoryAsInterest", () => 2);
    box.stub!.handle("confirmMemory", () => undefined);
    box.stub!.handle("setInterestEnabled", () => undefined);
    box.stub!.handle("deleteInterest", () => undefined);
    const changed = vi.fn();
    const { host } = mount(<MemoryPane {...props} onChanged={changed} />);
    await flush();
    click(host, "确认"); await flush();
    expect(changed).not.toHaveBeenCalled();
    click(host, "添加为兴趣"); await flush();
    tab(host, "兴趣");
    act(() => host.querySelector<HTMLButtonElement>('[aria-label="兴趣 本地优先"]')!.click());
    await flush();
    click(host, "删除"); await flush();
    expect(changed).toHaveBeenCalledTimes(3);
  });
});
