import { threadFixture } from "./fixtures/chat";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import type { ModelChoice } from "../lib/api";
import { createApiStub, type ApiStub } from "./api";
import { createEventStub, type EventStub } from "./events";
import { aiPageFixture, bindingsFixture, modelPoolFixture, poolEntryFixture, providerFixture } from "./fixtures/settings";
import { click, clickText, handleShellReads, typeInto } from "./fixtures/shell";
import { createReactHarness } from "./react";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null, events: null as EventStub | null }));

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  const api = await import("./api");
  const events = await import("./events");
  return { ...actual, api: api.createApiProxy(() => box.stub), ...events.eventListenerExports(() => box.events) };
});

describe("model choice inside a conversation", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };
  let sends: Array<{ threadId: number; model: ModelChoice | null }>;

  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    sends = [];
    box.stub = createApiStub();
    box.events = createEventStub();
    handleShellReads(box.stub);
    box.stub.handle("threadList", () => ({ folders: [], threads: [threadFixture({ id: 1, title: "线程一", chat_model: { provider_id: 2, model: "qwen/qwen3-8b" } })], archived_count: 0 }));
    box.stub.handle("messages", () => ({ messages: [], has_more: false }));
    box.stub.handle("ai", () => aiPageFixture({ providers: [providerFixture({ id: 1, name: "本机" }), providerFixture({ id: 2, name: "云端" })] }));
    box.stub.handle("modelBindings", () => bindingsFixture({ tasks: [{ task: "chat", title: "对话", provider_id: 1, model: "m" }] }));
    box.stub.handle("modelPool", () => modelPoolFixture({ pool: [
      poolEntryFixture({ provider_id: 2, model: "qwen/qwen3-8b", chat: true }),
      poolEntryFixture({ provider_id: 1, model: "m-fast", chat: true }),
      poolEntryFixture({ provider_id: 1, model: "m", chat: true }),
      poolEntryFixture({ provider_id: 1, model: "embed-small" }),
    ] }));
    box.stub.handle("sendMessage", (threadId, _text, _citations, _digest, model) => {
      sends.push({ threadId, model: model ?? null });
      return threadId || 9;
    });
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
  });

  const chip = (host: HTMLElement) => host.querySelector<HTMLButtonElement>('button[aria-label="这条对话的模型"]')!;
  async function send(host: HTMLElement, text: string) {
    typeInto(host.querySelector<HTMLTextAreaElement>('textarea[aria-label="对话消息"]')!, text);
    click(host.querySelector('button[aria-label="发送"]'));
    await harness.flush();
  }

  it("restores a thread's own model, switches only that thread, and starts new threads from the default", async () => {
    const { host } = harness.mount(<App />);
    await harness.flush();
    clickText(host, "线程一");
    // 模型说明与思考档位分别加载，等两者就绪后再操作。
    await vi.waitFor(async () => {
      await harness.flush();
      expect(chip(host).textContent).toBe("qwen3-8boff");
    });
    expect(chip(host).title).toBe("qwen/qwen3-8b · 云端");

    click(chip(host));
    const options = [...host.querySelectorAll<HTMLButtonElement>('[aria-label="这条对话的模型"][role="dialog"] [role="option"]')];
    const texts = options.map((o) => o.textContent ?? "");
    expect(texts).toHaveLength(3);
    expect(texts.filter((t) => t.includes("默认"))).toEqual([expect.stringMatching(/^mm自动默认/)]);
    click(options[texts.findIndex((t) => t.startsWith("m-fast"))]);
    await harness.flush();
    expect(host.textContent).toContain("之后的回复改用 m-fast · 本机");
    await send(host, "继续");
    expect(sends.at(-1)).toEqual({ threadId: 1, model: { provider_id: 1, model: "m-fast" } });
    act(() => box.events!.emit("chat:done", 1));
    await harness.flush();

    clickText(host, "新对话");
    await harness.flush();
    expect(chip(host).textContent).toBe("moff");
    await send(host, "新的问题");
    expect(sends.at(-1)).toEqual({ threadId: 0, model: null });
  });
});
