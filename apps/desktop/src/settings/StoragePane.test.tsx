import type { StorageMessage } from "../bindings";
import { deferred } from "../test/deferred";
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

type FixtureExtra = Omit<Partial<StorageItemView>, "hint" | "confirm" | "passive"> & { hint?: string; confirm?: string[]; passive?: string; action_label?: string; confirm_title?: string };
const legacy = (text: string): StorageMessage => ({ code: "legacy_text", params: { text } });
function row(kind: StorageItemView["kind"], _label: string, bytes: number, extra: FixtureExtra = {}): StorageItemView {
  return {
    kind, hint: legacy(extra.hint ?? ""), bytes, free: extra.free ?? false, model: "", tables: [],
    action: extra.action ?? null, passive: extra.passive ? "follows_articles" : null, confirm: (extra.confirm ?? []).map(legacy),
  };
}
function page(overrides: Partial<Record<string, StorageItemView>> = {}): StoragePage {
  const items = [
    row("articles", "文章", 600, { hint: "38,210 篇" }),
    row("search_index", "全文索引", 440, { passive: "随文章变化" }),
    row("vectors", "向量", 200, { action: "old_vectors", action_label: "清除旧模型向量", confirm_title: "清除旧模型向量", confirm: ["删除 8,120 段。", "之后重新生成。"] }),
    row("ai_cache", "AI 缓存", 30, { action: "ai_cache", action_label: "清空", confirm_title: "清空 AI 缓存", confirm: ["删除 9,870 条。", "可能产生费用。"] }),
    row("free", "空闲空间", 118, { free: true, action: "vacuum", action_label: "压缩数据库", confirm_title: "压缩数据库", confirm: ["还给磁盘。"] }),
  ].map((item) => overrides[item.kind] ?? item);
  return {
    data_dir: "/data/rrss", total_bytes: Math.round(2.14 * 1024 ** 3), summary: legacy("数据库 1.41 GB · 媒体缓存 632 MB"),
    groups: [{ kind: "database", bytes: Math.round(1.41 * 1024 ** 3), note: legacy("含未写回的日志 12 MB"), items }],
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
      days: 0, default_days: 90, custom_feeds: 0, preview: null, preview_hint: null, can_run: false,
      first_run: false, confirm: [],
    }));
    box.stub.handle("backupPage", () => ({
      last_manual_at: null, schedule: "off",
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
    const gate = deferred<void>();
    box.stub!.handle("storageClean", async (action: string) => { seen.push(action); await gate.promise; return { code: "storage_cleaned" as const, params: { action: "ai_cache" as const, count: 9870 } }; });
    act(() => onAsk.mock.calls[0][2]());
    await flush();
    expect(item(host, "ai_cache").textContent).toContain("清理中…");
    expect(button(item(host, "vectors"), "清除旧模型向量").disabled).toBe(true);
    expect(button(host, "重新统计").disabled).toBe(true);
    box.stub!.handle("storagePage", () => page({ ai_cache: row("ai_cache", "AI 缓存", 0) }));
    client.setQueryData(diagnosticsKeys.page, { vec: "", stats: "", db: "", ai: "", log_hint: "" });
    gate.resolve(); await flush();
    expect(seen).toEqual(["ai_cache"]);
    expect(box.stub!.callCount("storagePage")).toBe(2);
    expect(host.querySelector('[role="status"]')?.textContent).toBe("已清空 AI 缓存（9,870 条）。");
    expect(item(host, "ai_cache").querySelector("button")).toBeNull();
    expect(client.getQueryState(diagnosticsKeys.page)?.isInvalidated).toBe(true);
  });

  it("notifies index consumers only after clearing old vectors", async () => {
    box.stub!.handle("storageClean", (action: string) => legacy(`done ${action}`));
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
