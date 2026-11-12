import { aiPageFixture, bindingsFixture, modelPoolFixture, providerFixture } from "./test/fixtures/settings";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { api } from "./lib/api";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  notifyManager.setScheduler(queueMicrotask);
});
afterAll(() => notifyManager.setScheduler((callback) => setTimeout(callback, 0)));
let cleanup = () => {};
afterEach(() => { cleanup(); vi.restoreAllMocks(); document.body.replaceChildren(); });
const fixture = aiPageFixture({
  providers: [providerFixture({ name: "本机", base_url: "http://127.0.0.1:1/v1" })], selected_id: 1,
});
async function flush() { await act(async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); }); }
function click(host: ParentNode, text: string) {
  const button = [...host.querySelectorAll("button")].find(el => el.textContent === text);
  expect(button).toBeTruthy();
  act(() => { button!.focus(); button!.click(); });
}
/** 按分类 id 点设置导航，改导航文字不影响用例。 */
function tab(host: ParentNode, id: string) {
  act(() => host.querySelector<HTMLButtonElement>(`[data-settings-tab="${id}"]`)!.click());
}
const bindings = bindingsFixture();
async function setup() {
  vi.spyOn(api, "ai").mockResolvedValue(structuredClone(fixture));
  vi.spyOn(api, "modelBindings").mockResolvedValue(structuredClone(bindings));
  vi.spyOn(api, "providerPresets").mockResolvedValue([]);
  vi.spyOn(api, "modelPool").mockResolvedValue(modelPoolFixture());
  vi.spyOn(api, "translatePage").mockRejectedValue(new Error("not needed"));
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host); act(() => root.render(<App />));
  cleanup = () => { act(() => root.unmount()); host.remove(); };
  await flush(); click(host, "设置"); await flush();
  tab(host, "model"); await flush();
  click(host, "编辑"); await flush();
  const input = host.querySelector<HTMLInputElement>('[role=dialog] input[placeholder="例如：本机 Ollama"]')!;
  expect(document.activeElement).toBe(input);
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "未保存的接口名");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await flush();
  return { host, input };
}

describe("Model settings leave protection", () => {

  it("blocks leaving during writes, keeps dirty edits on save failure, then saves and leaves", async () => {
    const { host, input } = await setup();
    let reject!: (error: Error) => void;
    vi.spyOn(api, "saveAiSettings").mockImplementation(() => new Promise((_, no) => { reject = no; }));
    click(host.querySelector("[role=dialog]")!, "保存"); await flush();
    tab(host, "translate"); await flush();
    expect(host.querySelector(".settings-nav [aria-current=page]")?.getAttribute("data-settings-tab")).toBe("model");
    expect(host.querySelector("[role=alertdialog]")).toBeNull();
    expect(host.textContent).toContain("本次操作完成后再离开");
    await act(async () => reject(new Error("database locked"))); await flush();
    expect(input.value).toBe("未保存的接口名");
    click(host, "返回阅读"); await flush();
    const confirm = host.querySelector<HTMLElement>("[role=alertdialog]")!;
    expect(confirm.textContent).toContain("编辑连接有未保存的修改");
    const saved = vi.spyOn(api, "saveAiSettings").mockResolvedValue(structuredClone(fixture));
    click(confirm, "保存并离开"); await flush();
    expect(saved).toHaveBeenCalledWith(expect.objectContaining({ provider: expect.objectContaining({ id: 1, name: "未保存的接口名" }) }));
    expect(host.querySelector("[role=alertdialog]")).toBeNull();
    expect(host.querySelector(".settings-nav")).toBeNull();
  });
});
