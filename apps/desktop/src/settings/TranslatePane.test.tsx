import { translatePageFixture } from "../test/fixtures/settings";
import { act, StrictMode } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { prefetchTranslateSettings, translateKeys } from "./translateQuery";
import { settingsChanged } from "./editState";
import type { SettingsEditState } from "./editState";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TranslatePage } from "../bindings";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import { TranslatePane } from "./TranslatePane";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});

const PAGE = translatePageFixture();
const harness = createReactHarness();
let client: QueryClient;
let stored: TranslatePage;
function mountPane(onEditStateChange?: (state: SettingsEditState | null) => void, onOpenModels = () => {}) {
  return harness.mount(<QueryClientProvider client={client}><StrictMode>
    <TranslatePane onEditStateChange={onEditStateChange} onOpenModels={onOpenModels} />
  </StrictMode></QueryClientProvider>);
}

function button(host: HTMLElement, text: string) {
  const found = [...host.querySelectorAll("button")].find((item) => item.textContent === text);
  expect(found).toBeTruthy();
  return found!;
}

function setInput(input: HTMLInputElement, value: string) {
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("translation setting feedback", () => {
  beforeEach(() => {
    client = new QueryClient();
    notifyManager.setScheduler(queueMicrotask);
    stored = structuredClone(PAGE);
    box.stub = createApiStub();
    box.stub.handle("translatePage", () => structuredClone(stored));
    box.stub.handle("saveTranslateSettings", (args) => {
      const { deepl_key, deeplx_token, http_key, ...values } = args;
      Object.assign(stored, values);
      for (const key of ["deepl_key", "deeplx_token", "http_key"] as const) {
        const value = args[key];
        if (typeof value === "string") stored[key] = { state: value ? "set" : "none", hint: value.length > 8 ? value.slice(-4) : "" };
      }
    });
    box.stub.handle("testTranslate", () => "示例译文");
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    notifyManager.setScheduler((callback) => setTimeout(callback, 0));
    box.stub?.assertNoUnexpectedCalls();
  });

  it("does not offer preview defaults as saved configuration when the first read fails", async () => {
    box.stub!.handle("translatePage", () => { throw new Error("unavailable"); });
    const { host } = mountPane();
    await harness.flush();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("读取翻译设置失败");
    expect(host.querySelector('input[aria-label="语言代码"]')).toBeNull();
    expect(box.stub!.callCount("saveTranslateSettings")).toBe(0);
    expect(button(host, "重试读取")).toBeTruthy();
    box.stub!.handle("translatePage", () => PAGE);
    act(() => button(host, "重试读取").click());
    await harness.flush();
    expect(host.querySelector<HTMLInputElement>('input[aria-label="语言代码"]')?.value).toBe("zh");
    expect(box.stub!.callCount("translatePage")).toBe(2);
  });

  it("discards an old prefetch when saving and reopens from the authoritative snapshot", async () => {
    client.setQueryData(translateKeys.page, PAGE, { updatedAt: 1 });
    const oldRead = box.stub!.deferNext("translatePage");
    const preload = prefetchTranslateSettings(client);
    const { host, unmount } = mountPane();
    setInput(host.querySelector<HTMLInputElement>('input[aria-label="语言代码"]')!, "fr");
    act(() => button(host, "保存").click());
    await harness.flush();
    await act(async () => { oldRead.resolve({ ...PAGE, target_lang: "en" }); await preload; });
    await harness.flush();
    expect(client.getQueryData<TranslatePage>(translateKeys.page)?.target_lang).toBe("fr");
    expect(host.textContent).not.toContain("有未保存的修改");
    expect(box.stub!.callCount("translatePage")).toBe(2);
    unmount();
    const next = mountPane();
    expect(next.host.querySelector<HTMLInputElement>('input[aria-label="语言代码"]')?.value).toBe("fr");
    expect(box.stub!.callCount("translatePage")).toBe(2);
  });

  it("updates clean fields on invalidation but only the snapshot while a draft is dirty", async () => {
    const onEdit = vi.fn();
    const { host } = mountPane(onEdit);
    await harness.flush();
    const input = host.querySelector<HTMLInputElement>('input[aria-label="语言代码"]')!;
    stored.target_lang = "en";
    await act(async () => { await settingsChanged(client); await prefetchTranslateSettings(client); });
    await harness.flush();
    expect(input.value).toBe("en");
    expect(onEdit).toHaveBeenLastCalledWith(expect.objectContaining({ label: "翻译设置", dirty: false, busy: false }));
    expect(host.textContent).not.toContain("有未保存的修改");
    expect(host.textContent).not.toContain("计费用量");
    setInput(input, "fr");
    expect(host.querySelector(".settings-savebar")?.textContent).toContain("有未保存的修改");
    expect(host.querySelector(".translate-pane")?.firstElementChild?.classList.contains("form-status")).toBe(false);
    stored.target_lang = "ja";
    await act(async () => { await settingsChanged(client); await prefetchTranslateSettings(client); });
    await harness.flush();
    expect(input.value).toBe("fr");
    expect(client.getQueryData<TranslatePage>(translateKeys.page)?.target_lang).toBe("ja");
    expect(onEdit).toHaveBeenLastCalledWith(expect.objectContaining({ label: "翻译设置", dirty: true, busy: false }));
  });

  it("blocks duplicate writes across mounts and skips prefetch during a write", async () => {
    const save = box.stub!.deferNext("saveTranslateSettings");
    const first = mountPane();
    const second = mountPane();
    await harness.flush();
    act(() => { button(first.host, "测试连接").click(); button(second.host, "测试连接").click(); });
    await harness.flush();
    const reads = box.stub!.callCount("translatePage");
    await prefetchTranslateSettings(client);
    expect(box.stub!.callCount("translatePage")).toBe(reads);
    expect(box.stub!.callCount("saveTranslateSettings")).toBe(1);
    await act(async () => save.resolve(undefined));
    await harness.flush();
    // 只有先点的那次走完保存与测试；第二个挂载的点击被写入互斥挡掉。
    expect(box.stub!.callCount("saveTranslateSettings")).toBe(1);
    expect(box.stub!.callCount("testTranslate")).toBe(1);
  });

  it("keeps entered values if the read after saving fails", async () => {
    const { host } = mountPane();
    await harness.flush();
    const input = host.querySelector<HTMLInputElement>('input[aria-label="语言代码"]')!;
    setInput(input, "de");
    box.stub!.handle("translatePage", () => { throw new Error("read failed"); });
    act(() => button(host, "保存").click());
    await harness.flush();
    expect(input.value).toBe("de");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("当前输入已保留");
    expect(host.querySelector("fieldset")!.disabled).toBe(false);
    expect(box.stub!.callCount("translatePage")).toBe(2);
    expect(client.getQueryData(translateKeys.page)).toBeUndefined();
    expect(host.textContent).not.toContain("有未保存的修改");
    harness.cleanup();
    const next = mountPane();
    await harness.flush();
    expect(next.host.querySelector('input')).toBeNull();
    expect(box.stub!.callCount("translatePage")).toBe(2);
  });

  it("saves a picked option alone and keeps unsaved typing in the draft", async () => {
    const onEdit = vi.fn();
    const { host } = mountPane(onEdit);
    await harness.flush();
    const url = host.querySelector<HTMLInputElement>('input[aria-label="HTTP 服务地址"]')!;
    setInput(url, "http://10.0.0.2:5000");
    act(() => button(host, "英语").click());
    await harness.flush();
    expect(box.stub!.callCount("saveTranslateSettings")).toBe(1);
    expect(stored).toMatchObject({ target_lang: "en", http_url: "http://127.0.0.1:5000" });
    expect(url.value).toBe("http://10.0.0.2:5000");
    expect(host.querySelector<HTMLInputElement>('input[aria-label="语言代码"]')!.value).toBe("en");
    expect(onEdit).toHaveBeenLastCalledWith(expect.objectContaining({ dirty: true }));
    // 离开确认里的「保存并离开」提交剩下的草稿。
    await act(async () => { expect(await onEdit.mock.lastCall![0].save()).toBe(true); });
    await harness.flush();
    expect(stored).toMatchObject({ target_lang: "en", http_url: "http://10.0.0.2:5000" });
    expect(onEdit).toHaveBeenLastCalledWith(expect.objectContaining({ dirty: false }));
  });
});
