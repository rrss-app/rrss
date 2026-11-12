import { act, type ComponentProps } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import type { TestClock } from "../test/clock";
import type { StorageItemView, StoragePage } from "../lib/api";
import { StoragePane } from "./StoragePane";
import { diagnosticsKeys } from "./diagnosticsQuery";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
const harness = createReactHarness();

function row(key: string, label: string, bytes: number, extra: Partial<StorageItemView> = {}): StorageItemView {
  return {
    key, label, hint: "", bytes, size: `${bytes} MB`, free: false,
    action: "", action_label: "", passive: "", confirm_title: "", confirm: [], ...extra,
  };
}
function page(overrides: Partial<Record<string, StorageItemView>> = {}): StoragePage {
  const items = [
    row("articles", "文章", 600, { hint: "38,210 篇" }),
    row("search_index", "全文索引", 440, { passive: "随文章变化" }),
    row("vectors", "向量", 200, { action: "old_vectors", action_label: "清除旧模型向量", confirm_title: "清除旧模型向量", confirm: ["删除 8,120 段。", "之后重新生成。"] }),
    row("ai_cache", "AI 缓存", 30, { action: "ai_cache", action_label: "清空", confirm_title: "清空 AI 缓存", confirm: ["删除 9,870 条。", "可能产生费用。"] }),
    row("free", "空闲空间", 118, { free: true, action: "vacuum", action_label: "压缩数据库", confirm_title: "压缩数据库", confirm: ["还给磁盘。"] }),
  ].map((item) => overrides[item.key] ?? item);
  return {
    data_dir: "/data/rrss", total: "2.14 GB", summary: "数据库 1.41 GB · 媒体缓存 632 MB",
    groups: [{ key: "database", label: "数据库", size: "1.41 GB", note: "含未写回的日志 12 MB", items }],
    scanned_at: Date.UTC(2026, 8, 27, 6, 32),
  };
}

let client: QueryClient;
let clock: TestClock;
function mount(props: Partial<ComponentProps<typeof StoragePane>> = {}) {
  const onAsk = props.onAsk ?? vi.fn((_t: string, _m: string, ok: () => void) => ok());
  const view = harness.mount(<QueryClientProvider client={client}><StoragePane onAsk={onAsk} {...props} /></QueryClientProvider>);
  return { ...view, onAsk };
}
async function flush() { await harness.flush(); await clock.advance(1); await harness.flush(); }
function button(host: HTMLElement, text: string) { return [...host.querySelectorAll("button")].find((item) => item.textContent === text)!; }
function item(host: HTMLElement, key: string) { return host.querySelector<HTMLElement>(`[data-storage-item="${key}"]`)!; }

describe("storage page", () => {
  beforeEach(() => {
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    clock = harness.useClock();
    client = new QueryClient();
    box.stub = createApiStub();
    box.stub.handle("storagePage", () => page());
    box.stub.handle("retentionPage", () => ({
      days: 0, default_days: 90, custom_feeds: 0, preview: "", preview_hint: "", can_run: false,
      confirm_title: "立即清理旧文章正文", confirm: [], confirm_ok: "清理",
    }));
    box.stub.handle("backupPage", () => ({
      last_manual: "还没有手动备份过。", schedule: "off", auto_summary: "升级前自动 · 定时关闭",
      entries: [], restore_notice: null, rules: 0, interests: 0, memories: 0,
    }));
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    vi.unstubAllGlobals();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("confirms, cleans once, re-reads and reports the host message", async () => {
    const onAsk = vi.fn();
    const { host } = mount({ onAsk }); await flush();
    act(() => button(item(host, "ai_cache"), "清空").click());
    expect(onAsk).toHaveBeenCalledWith("清空 AI 缓存", "删除 9,870 条。\n可能产生费用。", expect.any(Function));
    expect(box.stub!.callCount("storageClean")).toBe(0);

    const seen: string[] = [];
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    box.stub!.handle("storageClean", async (action: string) => { seen.push(action); await gate; return "已清空 AI 缓存（9,870 条）。"; });
    act(() => onAsk.mock.calls[0][2]());
    await flush();
    expect(item(host, "ai_cache").textContent).toContain("清理中…");
    expect(button(item(host, "vectors"), "清除旧模型向量").disabled).toBe(true);
    expect(button(host, "重新统计").disabled).toBe(true);
    box.stub!.handle("storagePage", () => page({ ai_cache: row("ai_cache", "AI 缓存", 0) }));
    client.setQueryData(diagnosticsKeys.page, { vec: "", stats: "", db: "", ai: "", log_hint: "" });
    release(); await flush();
    expect(seen).toEqual(["ai_cache"]);
    expect(box.stub!.callCount("storagePage")).toBe(2);
    expect(host.querySelector('[role="status"]')?.textContent).toBe("已清空 AI 缓存（9,870 条）。");
    expect(item(host, "ai_cache").querySelector("button")).toBeNull();
    expect(client.getQueryState(diagnosticsKeys.page)?.isInvalidated).toBe(true);
  });

  it("notifies index consumers only after clearing old vectors", async () => {
    box.stub!.handle("storageClean", (action: string) => `done ${action}`);
    const changed = vi.fn();
    const editState = vi.fn();
    const { host } = mount({ onChanged: changed, onEditStateChange: editState }); await flush();
    act(() => button(item(host, "ai_cache"), "清空").click()); await flush();
    expect(changed).not.toHaveBeenCalled();
    act(() => button(item(host, "vectors"), "清除旧模型向量").click()); await flush();
    expect(changed).toHaveBeenCalledTimes(1);
    expect(editState).toHaveBeenCalledWith(expect.objectContaining({ label: "存储清理", busy: true }));
    expect(editState).toHaveBeenLastCalledWith(expect.objectContaining({ busy: false }));
  });

});
