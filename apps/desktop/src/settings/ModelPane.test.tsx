import { aiPageFixture, applyPoolChange, bindingsFixture, modelPoolFixture, poolEntryFixture, providerFixture, translatePageFixture } from "../test/fixtures/settings";
import { act, StrictMode } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AiPage, ModelBindingsView, ModelPoolView, ProviderPreset, SaveAiSettingsArgs, SaveModelPoolArgs } from "../bindings";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import type { SettingsEditState } from "./editState";
import { ModelPane } from "./ModelPane";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});

const PRESETS: ProviderPreset[] = [
  { id: "openai", name: "OpenAI", base_url: "https://api.openai.com/v1", protocol: "chat", compat_json: "{}" },
  { id: "ollama", name: "Ollama（本机）", base_url: "http://127.0.0.1:11434/v1", protocol: "chat", compat_json: "{}" },
];
const task = (name: string, title: string, provider_id = 0, model = "") => ({ task: name, title, provider_id, model });

const harness = createReactHarness();
let client: QueryClient;
let page: AiPage;
let view: ModelBindingsView;
let saves: SaveAiSettingsArgs[];
let poolSaves: SaveModelPoolArgs[];
let pool: ModelPoolView;

function mountPane(props: { onAsk?: ReturnType<typeof vi.fn>; onEdit?: (s: SettingsEditState | null) => void } = {}) {
  return harness.mount(<QueryClientProvider client={client}><StrictMode>
    <ModelPane onAsk={props.onAsk ?? vi.fn()} onNavigate={vi.fn()} onEditStateChange={props.onEdit} />
  </StrictMode></QueryClientProvider>);
}
function button(root: ParentNode, text: string) {
  const found = [...root.querySelectorAll("button")].find((b) => b.textContent === text || b.getAttribute("aria-label") === text);
  expect(found, text).toBeTruthy();
  return found!;
}
function type(el: HTMLInputElement | HTMLSelectElement, value: string) {
  act(() => {
    const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
    el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
  });
}
const click = (el: HTMLElement) => act(() => el.click());
const dialog = (host: HTMLElement) => host.querySelector<HTMLElement>('[role="dialog"]');
const checkbox = (root: HTMLElement, model: string) =>
  [...root.querySelectorAll<HTMLLabelElement>(".catalog-list label")].find((l) => l.textContent?.includes(model))!.querySelector("input")!;

describe("ModelPane", () => {
  beforeEach(() => {
    client = new QueryClient();
    notifyManager.setScheduler(queueMicrotask);
    page = aiPageFixture();
    view = bindingsFixture({ tasks: [task("focus_summary", "焦点摘要"), task("article_translate", "本篇翻译"), task("chat", "对话")] });
    saves = [];
    poolSaves = [];
    pool = modelPoolFixture();
    const stub = createApiStub();
    stub.handle("modelPool", () => structuredClone(pool));
    stub.handle("saveModelPool", (args) => {
      poolSaves.push(structuredClone(args));
      pool = applyPoolChange(pool, args, view);
      return structuredClone(pool);
    });
    stub.handle("ai", () => structuredClone(page));
    stub.handle("modelBindings", () => structuredClone(view));
    stub.handle("providerPresets", () => structuredClone(PRESETS));
    stub.handle("translatePage", () => translatePageFixture({ backend: "deepl" }));
    stub.handle("vectorSpaceStatus", () => ({
      effective: { id: 1, model: "old", provider_name: "OpenAI", dims: 3, source_dims: 3, state: "active", chunks: 10, stale_reason: null, checked_at: null },
      building: null, allow_model_change: true, dims_setting: "auto", estimate_chunks: 10, estimate_chars: 1000, estimate_tokens: 500, local: false,
    }));
    stub.handle("saveAiSettings", (args) => {
      saves.push(structuredClone(args));
      const id = args.provider.id || page.providers.length + 1;
      const saved = providerFixture({ id, kind: args.provider.kind ?? "openai_compat", name: args.provider.name,
        base_url: args.provider.base_url, key: args.provider.api_key ? "set" : "none", key_hint: args.provider.api_key?.slice(-4) ?? "" });
      page = { ...page, providers: [...page.providers.filter((p) => p.id !== id), saved], selected_id: id };
      return structuredClone(page);
    });
    box.stub = stub;
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    notifyManager.setScheduler((callback) => setTimeout(callback, 0));
    box.stub?.assertNoUnexpectedCalls();
  });

  it("stages add-model changes, keeps the remembered catalog when a refetch fails, and asks for a replacement before removing a model in use", async () => {
    page = aiPageFixture({ providers: [providerFixture({ id: 2, name: "OpenRouter" })] });
    view.tasks[0] = task("focus_summary", "焦点摘要", 2, "openai/gpt-4.1");
    pool = modelPoolFixture({
      pool: [poolEntryFixture({ provider_id: 2, model: "openai/gpt-4.1", uses: ["focus_summary"] }), poolEntryFixture({ provider_id: 2, model: "qwen/qwen3" })],
      catalogs: [{ provider_id: 2, models: ["openai/gpt-4.1", "qwen/qwen3", "openai/o3"], fetched_at: 1 }],
    });
    box.stub!.handle("fetchModelCatalog", () => { throw new Error("服务返回 401"); });
    const onAsk = vi.fn();
    const edits: Array<SettingsEditState | null> = [];
    const { host } = mountPane({ onAsk, onEdit: (s) => edits.push(s) });
    await harness.flush();
    click(button(host, "添加模型"));
    const sheet = dialog(host)!;
    click(button(sheet, "重新获取"));
    await harness.flush();
    expect(sheet.textContent).toContain("重新获取失败，已记住的 3 个模型不受影响");
    expect(sheet.querySelectorAll(".catalog-list input")).toHaveLength(3);

    type(sheet.querySelector<HTMLInputElement>('input[aria-label="手填模型 id"]')!, "my/private-model");
    click(button(sheet, "加入勾选"));
    click(checkbox(sheet, "openai/o3"));
    click(checkbox(sheet, "openai/gpt-4.1"));
    expect(edits.at(-1)).toMatchObject({ dirty: true });
    expect(sheet.textContent).toContain("将加入 2 个、移出 1 个");
    const apply = button(sheet, "应用");
    click(apply);
    // 移出的模型正用于焦点摘要：先问改用哪个，同一次写入带上替换。
    const prompt = sheet.querySelector<HTMLElement>('[role="group"][aria-label="改用哪个模型"]')!;
    expect(prompt.textContent).toContain("焦点摘要");
    expect(poolSaves).toHaveLength(0);
    click(button(prompt, "移出"));
    await harness.flush();
    expect(poolSaves).toEqual([{
      add: [{ provider_id: 2, model: "my/private-model" }, { provider_id: 2, model: "openai/o3" }],
      remove: [{ provider_id: 2, model: "openai/gpt-4.1" }],
      replace: { provider_id: 2, model: "qwen/qwen3" },
    }]);
    expect(dialog(host)).toBeNull();
    expect(host.textContent).toContain("已加入 2 个、移出 1 个模型，1 项用途改用替换模型。");
    expect(view.tasks[0]).toMatchObject({ provider_id: 2, model: "qwen/qwen3" });
    expect(onAsk).not.toHaveBeenCalled();
  });

  it("offers save-and-leave for a new connection, then goes straight to picking models", async () => {
    const onAsk = vi.fn();
    const { host } = mountPane({ onAsk });
    await harness.flush();
    click(button(host, "添加连接"));
    const sheet = dialog(host)!;
    type(sheet.querySelector<HTMLInputElement>('input[placeholder="例如：本机 Ollama"]')!, "工作用 OpenAI");
    type(sheet.querySelector<HTMLInputElement>('input[placeholder="sk-…"]')!, "sk-test-1234");
    click(button(sheet, "取消"));
    expect(onAsk).toHaveBeenCalledTimes(1);
    const [title, , ok, options] = onAsk.mock.calls[0];
    expect(title).toBe("添加连接有未保存的修改");
    expect(options).toMatchObject({ okLabel: "保存并离开", cancelLabel: "继续编辑", secondary: { label: "不保存" } });
    await act(async () => ok());
    await harness.flush();
    expect(saves).toHaveLength(1);
    expect(saves[0].provider).toMatchObject({ id: 0, kind: "openai", name: "工作用 OpenAI", api_key: "sk-test-1234", base_url: "https://api.openai.com/v1" });
    const next = dialog(host)!;
    expect(next.querySelector("h3")?.textContent).toBe("添加模型");
    expect(next.textContent).toContain("已保存连接「工作用 OpenAI」。勾选要用的模型。");
  });

  it("reports a failed reread after delete without claiming the delete failed", async () => {
    page.providers = [providerFixture({ id: 1, name: "旧连接" })];
    box.stub!.handle("deleteProvider", () => {
      page = { ...page, providers: [] };
      box.stub!.handle("ai", () => { throw new Error("database is locked"); });
    });
    const onAsk = vi.fn();
    const { host } = mountPane({ onAsk });
    await harness.flush();
    click(button(host, "编辑 旧连接"));
    click(button(dialog(host)!, "删除连接"));
    await act(async () => onAsk.mock.calls[0][2]());
    await harness.flush();
    expect(dialog(host)).toBeNull();
    expect(host.textContent).toContain("已删除「旧连接」，但列表刷新失败");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("刷新模型设置失败");
    expect(box.stub!.callCount("deleteProvider")).toBe(1);
  });
  it("keeps the binding when rebuild is refused and displays the backend message", async () => {
    page.providers = [providerFixture({ id: 1 })];
    view.tasks = [task("embed", "嵌入", 1, "old")];
    pool = modelPoolFixture({ pool: [poolEntryFixture({ provider_id: 1, model: "old" }), poolEntryFixture({ provider_id: 1, model: "next" })] });
    box.stub!.handle("saveModelBindings", () => { throw { kind: "vector_space_rebuild_required", message: "换模型需要重建向量库" }; });
    const onAsk = vi.fn();
    const { host } = mountPane({ onAsk });
    await harness.flush();
    click(button(host, "嵌入的模型"));
    const option = [...host.querySelectorAll<HTMLElement>('[role="option"]')].find(e => e.textContent?.includes("next"));
    expect(option).toBeTruthy();
    click(option!);
    await act(async () => onAsk.mock.calls[0][2]());
    await harness.flush();
    expect(view.tasks[0].model).toBe("old");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("换模型需要重建向量库");
  });

});
