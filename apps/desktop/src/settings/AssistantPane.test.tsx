import { piProbeFixture, mcpFixture, aiPageFixture, bindingsFixture, modelPoolFixture, poolEntryFixture, providerFixture } from "../test/fixtures/settings";
import { act, StrictMode } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ModelBindingsView, PiPage } from "../bindings";
import type { ApiStub } from "../test/api";
import { createPiFixture, FIXTURE_PI_PAGE } from "../test/fixtures/pi";
import { createReactHarness } from "../test/react";
import { aiKeys } from "./aiKeys";
import { AssistantPane } from "./AssistantPane";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub), onPiInstall: () => Promise.resolve(() => {}) };
});

const harness = createReactHarness();
let client: QueryClient;
let page: PiPage;
let bindings: ModelBindingsView;

function mountPane(onAsk = vi.fn(), onNavigate = vi.fn()) {
  const mounted = harness.mount(<QueryClientProvider client={client}><StrictMode>
    <AssistantPane onAsk={onAsk} onNavigate={onNavigate} />
  </StrictMode></QueryClientProvider>);
  return { ...mounted, onAsk, onNavigate };
}
function button(root: ParentNode, name: string) {
  const found = [...root.querySelectorAll("button")].find((b) => b.textContent === name || b.getAttribute("aria-label") === name);
  expect(found, name).toBeTruthy();
  return found!;
}
function type(el: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement, value: string) {
  act(() => {
    const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype
      : el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
    el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
  });
}
const click = (el: HTMLElement) => act(() => el.click());
const dialog = (host: HTMLElement) => host.querySelector<HTMLElement>('[role="dialog"]');

describe("AssistantPane", () => {
  beforeEach(() => {
    client = new QueryClient();
    notifyManager.setScheduler(queueMicrotask);
    page = {
      ...structuredClone(FIXTURE_PI_PAGE),
      thinking_levels: ["off", "low"],
      tools: [{ name: "get_article", label: "读取原文", description: "只读前 3 段", default_description: "按 ID 读取正文块", enabled: true, confirm: false }],
      skills: [{ name: "notes", description: "整理笔记", bundled: false, enabled: true, loaded: false }],
      mcp_servers: [mcpFixture({ id: 7, name: "files", args: ["--stdio"] })],
    };
    bindings = bindingsFixture({ tasks: [{ task: "chat", title: "对话", provider_id: 1, model: "qwen2.5:14b" }] });
    const { stub } = createPiFixture();
    stub.handle("pi", () => structuredClone(page));
    stub.handle("modelBindings", () => structuredClone(bindings));
    const known = (tool: "supported" | "unsupported") => ({ tool_calling: tool, structured_output: "supported", image_input: "unknown", reasoning: "supported" } as const);
    stub.handle("modelPool", () => modelPoolFixture({ pool: [
      poolEntryFixture({ provider_id: 2, model: "anthropic/claude-sonnet-4.5", chat: true, thinking: "medium", detected: known("supported") }),
      poolEntryFixture({ provider_id: 1, model: "qwen2.5:14b", chat: true, thinking: "low", detected: known("unsupported") }),
      poolEntryFixture({ provider_id: 1, model: "nomic-embed-text" }),
    ] }));
    stub.handle("ai", () => aiPageFixture({ providers: [providerFixture({ id: 1, name: "本机" }), providerFixture({ id: 2, name: "云端" })], selected_id: 1 }));
    box.stub = stub;
    // 注册表选项只在宿主里读写；这里走桩。
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  });
  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    harness.cleanup();
    client.clear();
    notifyManager.setScheduler((callback) => setTimeout(callback, 0));
    box.stub?.assertNoUnexpectedCalls();
  });

  it("retries an installation failure delivered as a typed error", async () => {
    box.stub!.handle("piProbe", () => piProbeFixture({ version_state: "missing" }));
    box.stub!.handle("piInstallStatus", () => ({ state: "failed", received: 0, total: null, dir: "", message: "", error: { code: "pi_package_binary_missing" as const, params: { file: "pi" }, detail: "fixture" } }));
    box.stub!.handle("installPi", () => ({ state: "downloading", received: 0, total: null, dir: "", message: "" }));
    const { host } = mountPane();
    await harness.flush();
    const status = host.querySelector<HTMLElement>(".engine-status[role=alert]")!;
    expect(status.dataset.errorCode).toBe("pi_package_binary_missing");
    click(status.querySelector<HTMLButtonElement>("button:last-child")!);
    await harness.flush();
    expect(host.querySelector(".engine-status[aria-busy=true]")).not.toBeNull();
    expect(host.querySelector(".engine-status[data-error-code]")).toBeNull();
  });

  it("saves the chat model immediately into the shared bindings snapshot and lists only chat models", async () => {
    box.stub!.handle("saveModelBindings", (args) => {
      bindings = { ...bindings, tasks: bindings.tasks.map((t) => ({ ...t, ...args.bindings[0] })) };
      return structuredClone(bindings);
    });
    const { host, onNavigate } = mountPane();
    await harness.flush();
    expect(host.textContent).toContain("这个模型不能调用工具");
    expect(host.textContent).toContain("qwen2.5:14b · low");
    click(button(host, "对话模型"));
    const menu = host.querySelector<HTMLElement>('[role="dialog"][aria-label="对话模型"]')!;
    const options = [...menu.querySelectorAll('[role="option"]')].map((o) => o.textContent);
    expect(options).toHaveLength(2);
    expect(options.join()).not.toContain("nomic-embed-text");
    click([...menu.querySelectorAll<HTMLButtonElement>('[role="option"]')].find((o) => o.textContent?.includes("claude-sonnet-4.5"))!);
    await harness.flush();
    expect(box.stub!.callCount("saveModelBindings")).toBe(1);
    expect(client.getQueryData<ModelBindingsView>(aiKeys.bindings)?.tasks[0]).toMatchObject({ provider_id: 2, model: "anthropic/claude-sonnet-4.5" });
    expect(host.textContent).not.toContain("这个模型不能调用工具");
    expect(host.textContent).toContain("claude-sonnet-4.5 · med");
    expect(button(host, "对话模型").textContent).toMatch(/^claude-sonnet-4\.5( |$)/);
    click(button(host, "在模型设置里改"));
    expect(onNavigate).toHaveBeenCalledWith("model", "model-pool");
  });

  it("edits tool notes and skills in dialogs", async () => {
    const calls: unknown[] = [];
    box.stub!.handle("setPiToolDescription", (name, text) => { calls.push(["tool", name, text]); });
    box.stub!.handle("piSkill", (name) => ({ name, description: "整理笔记", body: "先列要点" }));
    box.stub!.handle("saveSkill", (draft) => { calls.push(["skill", draft]); });
    const { host } = mountPane();
    await harness.flush();

    click(button(host, "编辑说明 读取原文"));
    click(button(dialog(host)!, "恢复默认"));
    expect(dialog(host)!.querySelector("textarea")!.value).toBe("按 ID 读取正文块");
    click(button(dialog(host)!, "保存"));
    await harness.flush();

    click(button(host, "编辑技能 notes"));
    await harness.flush();
    const body = dialog(host)!.querySelectorAll("textarea")[0];
    expect(body.value).toBe("先列要点");
    type(body, "先列要点，再写结论");
    click(button(dialog(host)!, "保存"));
    await harness.flush();

    expect(calls).toEqual([
      ["tool", "get_article", "按 ID 读取正文块"],
      ["skill", { name: "notes", description: "整理笔记", body: "先列要点，再写结论" }],
    ]);
    expect(dialog(host)).toBeNull();
  });

  it("keeps a busy restart visible", async () => {
    box.stub!.handle("restartPi", () => { throw new Error("回复尚未结束，请稍后重启引擎"); });
    const { host } = mountPane();
    await harness.flush();
    click(button(host, "重启引擎"));
    await harness.flush();
    expect(host.querySelector('[role="alert"].notice')?.textContent).toContain("回复尚未结束");
    expect(host.textContent).not.toContain("启动 Pi");
  });
});
