import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import type { TestClock } from "../test/clock";
import { deferred } from "../test/deferred";
import type { SettingsEditState } from "./editState";
import { mergeEditStates } from "./editState";
import { createSettingsSchema, formatMinutes, parseMinutes } from "./registry";
import { prefetchSettingsSchema, settingsKeys, settingsSchemaOptions } from "./settingsQuery";
import { SettingsRows } from "./SettingsRows";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
const harness = createReactHarness();

let client: QueryClient;
let clock: TestClock;
async function flush() { await harness.flush(); await clock.advance(1); await harness.flush(); }
const button = (host: HTMLElement, text: string) => [...host.querySelectorAll("button")].find((b) => b.textContent === text)!;
function setInput(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), "value")!.set!;
  act(() => { setter.call(el, value); el.dispatchEvent(new Event("input", { bubbles: true })); });
}
function mount(ids: Parameters<typeof SettingsRows>[0]["ids"], onEdit?: (s: SettingsEditState | null) => void) {
  return harness.mount(
    <QueryClientProvider client={client}>
      <SettingsRows ids={ids} label="测试设置" onEditStateChange={onEdit} />
    </QueryClientProvider>,
  );
}
const value = (id: string) => client.getQueryData<ReturnType<typeof createSettingsSchema>>(settingsKeys.schema)?.values.find((v) => v.id === id)?.value;

describe("settings rows", () => {
  beforeEach(() => {
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    clock = harness.useClock();
    client = new QueryClient();
    box.stub = createApiStub();
    box.stub.handle("settingsSchema", () => createSettingsSchema());
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    vi.unstubAllGlobals();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("writes switches, enums and times immediately and shows option hints from the backend", async () => {
    const writes: Array<[string, string]> = [];
    box.stub!.handle("setSetting", (id: string, v: string) => { writes.push([id, v]); return { id, value: v }; });
    const { host } = mount(["pi_auto_update", "ai_input_tier", "digest_generate_minutes"]);
    await flush();
    expect(host.textContent).toContain("均衡：每篇摘要");
    act(() => host.querySelector<HTMLButtonElement>('button[aria-label="自动更新对话引擎"]')!.click());
    await flush();
    act(() => button(host, "高").click());
    await flush();
    setInput(host.querySelector<HTMLInputElement>('input[aria-label="简报自动生成时间"]')!, "07:30");
    await flush();
    expect(writes).toEqual([["pi_auto_update", "1"], ["ai_input_tier", "high"], ["digest_generate_minutes", "450"]]);
    expect(value("ai_input_tier")).toBe("high");
    expect(host.textContent).toContain("少拆分");
    expect(host.querySelector<HTMLInputElement>('input[aria-label="简报自动生成时间"]')!.value).toBe("07:30");
    expect(host.querySelector(".settings-savebar")).toBeNull();
  });

  it("rolls an immediate change back and says why when the write fails", async () => {
    box.stub!.handle("setSetting", () => { throw new Error("database is locked"); });
    const { host } = mount(["ai_input_tier"]);
    await flush();
    const before = value("ai_input_tier");
    const other = before === "high" ? "低" : "高";
    act(() => button(host, other).click());
    await flush();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("没能保存「每次送给模型的正文」：数据库正忙");
    expect(value("ai_input_tier")).toBe(before);
  });

  it("keeps numbers as a draft with a save bar that the leave guard sees", async () => {
    const saved: string[] = [];
    box.stub!.handle("setSetting", (id: string, v: string) => { saved.push(`${id}=${v}`); return { id, value: v }; });
    const edits: Array<SettingsEditState | null> = [];
    const { host } = mount(["hide_threshold"], (s) => edits.push(s));
    await flush();
    const input = host.querySelector<HTMLInputElement>('input[aria-label="隐藏阈值"]')!;
    setInput(input, "abc");
    expect(edits.at(-1)).toMatchObject({ label: "测试设置", dirty: true, busy: false });
    expect(typeof edits.at(-1)?.save).toBe("function");
    act(() => button(host, "保存").click());
    await flush();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("需要填写整数");
    expect(saved).toEqual([]);
    setInput(input, "20000");
    act(() => button(host, "保存").click());
    await flush();
    expect(saved).toEqual(["hide_threshold=20000"]);
    expect(host.querySelector(".settings-savebar")).toBeNull();
    expect(edits.at(-1)).toMatchObject({ label: "测试设置", dirty: false, busy: false });
    // 撤销回到已保存的值。
    setInput(input, "5");
    act(() => button(host, "撤销").click());
    expect(input.value).toBe("20000");
  });

  it("edits token budgets in units of 10k tokens and saves tokens", async () => {
    const saved: string[] = [];
    box.stub!.handle("setSetting", (id: string, v: string) => { saved.push(`${id}=${v}`); return { id, value: v }; });
    const { host } = mount(["ai_daily_budget_tokens"]);
    await flush();
    const input = host.querySelector<HTMLInputElement>('input[aria-label="生成日预算（万 token，0 = 不限额）"]')!;
    expect(input.value).toBe("20");
    // 敲到一半的小数留在输入框里，不被换算回整数吃掉。
    setInput(input, "1.");
    expect(input.value).toBe("1.");
    setInput(input, "1.5");
    act(() => button(host, "保存").click());
    await flush();
    expect(saved).toEqual(["ai_daily_budget_tokens=15000"]);
    expect(input.value).toBe("1.5");
    setInput(input, "3");
    act(() => button(host, "撤销").click());
    expect(input.value).toBe("1.5");
  });

  it("keeps the draft when the host rejects it", async () => {
    let sent = "";
    box.stub!.handle("setSetting", (_id, value) => { sent = value; throw new Error("需要在 0 到 604800 之间"); });
    const { host } = mount(["pi_idle_timeout_secs"]);
    await flush();
    // 库里是秒，界面按分钟编辑。
    const input = host.querySelector<HTMLInputElement>('input[aria-label="空闲多久释放（分钟）"]')!;
    expect(input.value).toBe("30");
    setInput(input, "60");
    act(() => button(host, "保存").click());
    await flush();
    expect(sent).toBe("3600");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("需要在 0 到 604800 之间");
    expect(input.value).toBe("60");
    expect(host.querySelector(".settings-savebar")).not.toBeNull();
  });

  it("uses apply independently of the control type", async () => {
    const schema = createSettingsSchema();
    schema.specs.find(s => s.id === "pi_auto_update")!.apply = "draft";
    schema.specs.find(s => s.id === "hide_threshold")!.apply = "immediate";
    box.stub!.handle("settingsSchema", () => schema);
    box.stub!.handle("setSetting", (id, value) => ({ id, value }));
    const { host } = mount(["pi_auto_update", "hide_threshold"]);
    await flush();
    act(() => host.querySelector<HTMLButtonElement>('button[aria-label="自动更新对话引擎"]')!.click());
    await flush();
    expect(box.stub!.callCount("setSetting")).toBe(0);
    expect(host.querySelector(".settings-savebar")).not.toBeNull();
    act(() => button(host, "保存").click());
    await flush();
    expect(value("pi_auto_update")).toBe("1");
    setInput(host.querySelector<HTMLInputElement>("input")!, "25");
    expect(value("hide_threshold")).toBe("-10");
    act(() => host.querySelector("input")!.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    await flush();
    expect(value("hide_threshold")).toBe("25");
    expect(host.querySelector(".settings-savebar")).toBeNull();
  });

  it("registers writes across instances and keeps normalized results after unmount despite late reads", async () => {
    const write = deferred<{ id: string; value: string }>();
    const before = deferred<ReturnType<typeof createSettingsSchema>>();
    const during = deferred<ReturnType<typeof createSettingsSchema>>();
    box.stub!.handle("setSetting", () => write.promise);
    const first = mount(["hide_threshold"]);
    const second = mount(["pi_auto_update"]);
    await flush();
    box.stub!.handle("settingsSchema", () => before.promise);
    const oldRead = client.fetchQuery({ ...settingsSchemaOptions(), staleTime: 0 });
    const oldResult = oldRead.catch(() => undefined);
    setInput(first.host.querySelector<HTMLInputElement>("input")!, "00025");
    act(() => {
      button(first.host, "保存").click();
      second.host.querySelector<HTMLButtonElement>('button[aria-label="自动更新对话引擎"]')!.click();
    });
    await flush();
    expect(box.stub!.callCount("setSetting")).toBe(1);
    expect(client.isMutating({ mutationKey: settingsKeys.write })).toBe(1);
    const reads = box.stub!.callCount("settingsSchema");
    await prefetchSettingsSchema(client);
    expect(box.stub!.callCount("settingsSchema")).toBe(reads);
    expect(second.host.querySelector<HTMLButtonElement>('button[aria-label="自动更新对话引擎"]')!.disabled).toBe(true);
    first.unmount();
    box.stub!.handle("settingsSchema", () => during.promise);
    const newRead = client.fetchQuery({ ...settingsSchemaOptions(), staleTime: 0 }).catch(() => undefined);
    write.resolve({ id: "hide_threshold", value: "25" });
    await flush();
    before.resolve(createSettingsSchema());
    during.resolve(createSettingsSchema());
    await Promise.all([oldResult, newRead]);
    await flush();
    expect(value("hide_threshold")).toBe("25");
    expect(client.isMutating({ mutationKey: settingsKeys.write })).toBe(0);
    const reentered = mount(["hide_threshold"]);
    await flush();
    expect(reentered.host.querySelector<HTMLInputElement>("input")!.value).toBe("25");
  });

  it("clears only committed drafts on partial failure and retries only remaining values", async () => {
    const writes: string[] = [];
    let fail = true;
    box.stub!.handle("setSetting", (id, value) => {
      writes.push(id);
      if (id === "focus_summary_neutral_limit" && fail) throw new Error("locked");
      return { id, value: String(Number(value)) };
    });
    const { host } = mount(["hide_threshold", "focus_summary_neutral_limit"]);
    await flush();
    const inputs = host.querySelectorAll<HTMLInputElement>("input");
    setInput(inputs[0], "00025");
    setInput(inputs[1], "3");
    act(() => button(host, "保存").click());
    await flush();
    expect(inputs[0].value).toBe("25");
    expect(inputs[1].value).toBe("3");
    expect(host.querySelector('[role="alert"]')).not.toBeNull();
    fail = false;
    act(() => button(host, "保存").click());
    await flush();
    expect(writes).toEqual(["hide_threshold", "focus_summary_neutral_limit", "focus_summary_neutral_limit"]);
    expect(host.querySelector(".settings-savebar")).toBeNull();
  });
});

describe("registry helpers", () => {
  it("formats minutes as a time and back", () => {
    expect(formatMinutes(8 * 60)).toBe("08:00");
    expect(formatMinutes(-5)).toBe("00:00");
    expect(parseMinutes("23:59")).toBe(23 * 60 + 59);
    expect(parseMinutes("24:00")).toBeNull();
  });

  it("merges page parts: busy first, then unsaved edits", () => {
    const clean = { label: "a", dirty: false, busy: false };
    const dirty = { label: "b", dirty: true, busy: false };
    const busy = { label: "c", dirty: false, busy: true };
    expect(mergeEditStates([clean, dirty])).toBe(dirty);
    expect(mergeEditStates([dirty, busy])).toBe(busy);
    expect(mergeEditStates([null, clean])).toBe(clean);
    expect(mergeEditStates([null])).toBeNull();
  });
});
