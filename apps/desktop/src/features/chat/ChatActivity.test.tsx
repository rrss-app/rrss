import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../../test/api";
import { createEventStub, type EventStub } from "../../test/events";
import { createReactHarness } from "../../test/react";
import { ChatActivity } from "./ChatActivity";
import { ThreadMessages } from "./ChatMessages";
import { useConversationRuntime } from "./useConversationRuntime";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null, events: null as EventStub | null }));
vi.mock("../../lib/api", async () => {
  const api = await import("../../test/api");
  const events = await import("../../test/events");
  return { api: api.createApiProxy(() => box.stub), ...events.eventListenerExports(() => box.events) };
});

type Runtime = ReturnType<typeof useConversationRuntime>;

function Probe({ onError, onFix, onChat }: { onError: (message: string, fix?: string) => void; onFix?: (tab: string) => void; onChat?: (chat: Runtime) => void }) {
  const chat = useConversationRuntime({ onError, onDone: () => {} });
  onChat?.(chat);
  return <>
    <ChatActivity store={chat.activity} />
    {onFix ? <ThreadMessages store={chat.store} turns={chat.turns} threadId={1} busy={false} onOpenUrl={() => {}} onFix={onFix} /> : null}
  </>;
}

describe("chat activity", () => {
  const harness = createReactHarness();
  beforeEach(() => {
    box.stub = createApiStub();
    box.events = createEventStub();
    box.stub.handle("messages", () => ({ messages: [], has_more: false }));
  });
  afterEach(() => {
    harness.cleanup();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("shows progress, asks for web access, surfaces the version notice once and clears on done", async () => {
    const answers: Array<[string, boolean]> = [];
    const acks: string[] = [];
    box.stub!.handle("confirmFetch", (id, ok) => { answers.push([id, ok]); });
    box.stub!.handle("acknowledgePiVersionNotice", (version) => { acks.push(version); });
    const { host } = harness.mount(<Probe onError={() => {}} />);
    await harness.flush();
    const emit = box.events!.emit;

    act(() => emit("chat:status", { thread_id: 3, notice: { code: "chat_remote_reason", params: { reason: "正在查阅本地文章" }, detail: "engine status" }, notice_version: "" }));
    expect(host.querySelector(".chat-status-line")?.textContent).toBe("正在查阅本地文章");

    act(() => emit("chat:confirm", { thread_id: 3, id: "c1", title: "读取网页", message: "https://example.test", server: "", tool: "", args: "", can_remember: false, timeout_ms: 120000 }));
    const bar = host.querySelector('[role="alertdialog"]')!;
    expect(bar.getAttribute("aria-label")).toBe("读取网页");
    expect(host.querySelector(".chat-status-line")).toBeNull();
    await act(async () => [...bar.querySelectorAll("button")].find((b) => b.textContent === "允许")!.click());
    await harness.flush();
    expect(answers).toEqual([["c1", true]]);
    expect(host.querySelector('[role="alertdialog"]')).toBeNull();

    act(() => emit("chat:status", { thread_id: 3, notice: { code: "chat_remote_reason", params: { reason: "对话引擎 0.99.0 不在已验证的版本线内" }, detail: "fixture notice" }, notice_version: "0.99.0" }));
    await harness.flush();
    expect(acks).toEqual(["0.99.0"]);
    act(() => [...host.querySelectorAll("button")].find((b) => b.textContent === "知道了")!.click());
    expect(host.querySelector(".chat-notice")).toBeNull();

    act(() => emit("chat:done", 3));
    await harness.flush();
    expect(host.textContent).toBe("");
  });

  it("keeps a turn's error at the reply position with its steps and a settings link until the next turn", async () => {
    const onError = vi.fn();
    const onFix = vi.fn();
    let chat: Runtime | null = null;
    box.stub!.handle("sendMessage", () => 1);
    const { host } = harness.mount(<Probe onError={onError} onFix={onFix} onChat={(value) => { chat = value; }} />);
    await harness.flush();
    const emit = box.events!.emit;
    const alert = () => host.querySelector<HTMLElement>(".chat-failure[role=alert]");
    const step = (start: boolean, error: boolean) => ({ id: "c1", kind: "tool", tool: "fetch_url", server: "", server_tool: "", detail: "", article_id: null, start, error });

    await act(async () => { await chat!.sendMessage(1, "上游挂了吗", [], null); });
    act(() => emit("chat:start", 1));
    act(() => emit("chat:status", { thread_id: 1, notice_version: "", step: step(true, false) }));
    act(() => emit("chat:status", { thread_id: 1, notice_version: "", step: step(false, true) }));
    act(() => emit("chat:error", { thread_id: 1, kind: "model", message: { code: "legacy_text", params: {}, detail: "529 overloaded_error" } }));
    act(() => emit("chat:done", 1));
    await harness.flush();
    expect(alert()?.textContent).toContain("模型服务出错");
    expect(alert()?.textContent).toContain("529 overloaded_error");
    // 没有回复：这一轮做过的步骤留在失败提示上方，出错的那步可展开看到。
    act(() => host.querySelector<HTMLButtonElement>(".chat-failure-turn .chat-trace-toggle")!.click());
    expect(host.querySelector(".chat-failure-turn li[data-error]")?.textContent).toContain("读取网页");
    expect(onError).not.toHaveBeenCalled();

    act(() => emit("chat:error", { thread_id: 1, kind: "config", message: { code: "pi_chat_model_unset", detail: "fixture config" } }));
    act(() => [...alert()!.querySelectorAll("button")].find((b) => b.textContent === "去设置")!.click());
    expect(onFix).toHaveBeenCalledWith("model");

    // 下一轮开始就撤下；不知道线程的错误退回通知。
    act(() => emit("chat:start", 1));
    expect(alert()).toBeNull();
    act(() => emit("chat:error", { thread_id: 0, kind: "not_installed", message: { code: "pi_not_installed", detail: "fixture missing" } }));
    expect(onError.mock.calls).toEqual([["未找到对话引擎：可以在设置里下载。", "assistant"]]);
  });
});
