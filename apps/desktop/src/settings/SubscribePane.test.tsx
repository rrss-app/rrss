import { act, type ComponentProps } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SubscribePane } from "./SubscribePane";
import { api, type FeedTree, type RulesPage } from "../lib/api";
import { PREVIEW_FEEDS } from "../lib/preview";
import { feedKeys, feedQueryOptions } from "../lib/feedQuery";
import { rulesKeys, rulesQueryOptions } from "./rulesQuery";
import { createReactHarness } from "../test/react";
import { deferred } from "../test/deferred";
import { createSettingsSchema } from "./registry";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return { ...actual, api: { ...actual.api, feeds: vi.fn(), saveFeed: vi.fn(), unsubscribe: vi.fn(), createFolder: vi.fn(), settingsSchema: vi.fn() } };
});
const harness = createReactHarness();
let client: QueryClient;
function mount(props: Partial<ComponentProps<typeof SubscribePane>> = {}) {
  return harness.mount(<QueryClientProvider client={client}><SubscribePane onAsk={() => {}} {...props} /></QueryClientProvider>);
}
const button = (host: HTMLElement, text: string) => [...host.querySelectorAll("button")].find((b) => b.textContent === text)!;
const uncategorizedRow = (host: HTMLElement) => [...host.querySelectorAll<HTMLElement>(".feed-row")]
  .find((row) => row.querySelector(".feed-title")?.textContent === PREVIEW_FEEDS.uncategorized[0].title)!;
const titleInput = (host: HTMLElement) => host.querySelector<HTMLInputElement>('input[placeholder="留空使用来源的默认名称"]')!;
function input(el: HTMLInputElement, value: string) {
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("SubscribePane feed edit", () => {
  beforeEach(() => {
    client = new QueryClient();
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    notifyManager.setScheduler(queueMicrotask);
    vi.mocked(api.feeds).mockResolvedValue(PREVIEW_FEEDS);
    // 页内的「显示网站图标」读注册表快照。
    vi.mocked(api.settingsSchema).mockResolvedValue(createSettingsSchema());
    vi.mocked(api.saveFeed).mockRejectedValue(new Error("not a url"));
    vi.mocked(api.unsubscribe).mockRejectedValue(new Error("busy"));
    vi.mocked(api.createFolder).mockResolvedValue(42);
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    notifyManager.setScheduler((callback) => setTimeout(callback, 0));
    vi.resetAllMocks();
    vi.unstubAllGlobals();
  });

  it("preserves the draft and baseline across a folder move and failed save", async () => {
    const onEdit = vi.fn();
    const { host, unmount } = mount({ onEditStateChange: onEdit });
    await harness.flush();
    act(() => button(uncategorizedRow(host), "编辑").click());
    input(titleInput(host), "未保存名称");
    const updated = structuredClone(PREVIEW_FEEDS);
    const feed = updated.uncategorized.shift()!;
    updated.folders.push({ id: 99, name: "新分类", label: "新分类", indent: 0, position: 0, feeds: [{ ...feed, folder_id: 99 }] });
    act(() => client.setQueryData(feedKeys.tree, updated));
    await harness.flush();
    expect(titleInput(host).value).toBe("未保存名称");
    expect(onEdit).toHaveBeenLastCalledWith({ label: "订阅设置", dirty: true, busy: false });
    act(() => button(host, "保存修改").click());
    await harness.flush();
    expect(api.saveFeed).toHaveBeenCalledWith({ id: feed.id, title: "未保存名称", url: null, folder_id: null, retention: null });
    expect(host.textContent).toContain("保存失败");
    expect(titleInput(host).value).toBe("未保存名称");
    act(() => button(host, "取消").click());
    expect(onEdit.mock.lastCall?.[0]).toMatchObject({ dirty: false, busy: false });
    unmount();
    expect(onEdit).toHaveBeenLastCalledWith(null);
  });

  it("reports an unsaved per-host download limit to the leave guard", async () => {
    const onEdit = vi.fn();
    const { host } = mount({ onEditStateChange: onEdit });
    await harness.flush();
    const limit = host.querySelector<HTMLInputElement>('input[aria-label="每个网站同时下载几个订阅（1–8）"]')!;
    expect(limit.value).toBe("4");
    input(limit, "6");
    expect(onEdit.mock.lastCall?.[0]).toMatchObject({ dirty: true });
  });

  it("saves only the changed fields once, reuses the result and rejects late reads", async () => {
    const changed = vi.fn();
    const saved = structuredClone(PREVIEW_FEEDS);
    saved.uncategorized[0].title = "改名后";
    vi.mocked(api.saveFeed).mockResolvedValueOnce(saved);
    const { host } = mount({ onChanged: changed });
    await harness.flush();
    expect(changed).not.toHaveBeenCalled();
    const oldRead = deferred<FeedTree>();
    vi.mocked(api.feeds).mockReturnValueOnce(oldRead.promise);
    client.setQueryData(rulesKeys.page("all"), { rules: [] });
    act(() => { void client.prefetchQuery({ ...feedQueryOptions(), staleTime: 0 }); });
    const feedLoads = vi.mocked(api.feeds).mock.calls.length;
    act(() => button(uncategorizedRow(host), "编辑").click());
    input(titleInput(host), "改名后");
    const save = button(host, "保存修改");
    act(() => { save.click(); save.click(); });
    await harness.flush();
    expect(api.saveFeed).toHaveBeenCalledTimes(1);
    expect(api.saveFeed).toHaveBeenCalledWith({ id: saved.uncategorized[0].id, title: "改名后", url: null, folder_id: null, retention: null });
    expect(api.feeds).toHaveBeenCalledTimes(feedLoads);
    expect(changed).toHaveBeenCalledTimes(1);
    expect(changed).toHaveBeenCalledWith(saved);
    await act(async () => { oldRead.resolve(PREVIEW_FEEDS); });
    expect(client.getQueryData(feedKeys.tree)).toEqual(saved);
    expect(client.getQueryState(rulesKeys.page("all"))?.isInvalidated).toBe(true);
    expect(titleInput(host)).toBeNull();
    expect(host.textContent).toContain("改名后");
  });

  it("keeps rule scopes invalidated when a pre-write read arrives late", async () => {
    const key = rulesKeys.page("all");
    const oldRules: RulesPage = { rules: [], presets: [], folders: [{ id: 1, label: "deleted folder" }], feeds: [], hide_threshold: -10 };
    const late = deferred<RulesPage>();
    const readRules = vi.spyOn(api, "rules").mockReturnValueOnce(late.promise).mockResolvedValueOnce({ ...oldRules, folders: [{ id: 2, label: "new folder" }] });
    const preload = client.fetchQuery({ ...rulesQueryOptions("all"), staleTime: 0 }).catch(() => undefined);
    const { host } = mount();
    await harness.flush();
    input(host.querySelector<HTMLInputElement>("#subscribe-folder")!, "新分类");
    act(() => button(host, "新建").click());
    await harness.flush();
    await act(async () => { late.resolve(oldRules); await preload; });
    expect(client.getQueryState(key)?.isInvalidated).toBe(true);
    await client.fetchQuery({ ...rulesQueryOptions("all"), staleTime: 0 });
    expect(readRules).toHaveBeenCalledTimes(2);
    expect(client.getQueryData<RulesPage>(key)?.folders).toEqual([{ id: 2, label: "new folder" }]);
  });

  it("shows one partial-success error and prevents repeating a committed unsubscribe until reread", async () => {
    let confirm = () => {};
    const changed = vi.fn();
    const { host, unmount } = mount({ onChanged: changed, onAsk: (_title, _message, onOk) => { confirm = onOk; } });
    await harness.flush();
    vi.mocked(api.unsubscribe).mockResolvedValueOnce(undefined);
    vi.mocked(api.feeds).mockRejectedValueOnce(new Error("read failed"));
    act(() => button(uncategorizedRow(host), "编辑").click());
    act(() => button(host, "删除").click());
    act(() => confirm());
    await harness.flush();
    expect.soft(host.querySelectorAll('[role=alert]')).toHaveLength(1);
    expect(host.textContent).toContain("刚才的修改已保存");
    act(() => button(host.querySelectorAll<HTMLElement>(".feed-row")[0], "编辑").click());
    act(() => button(host, "删除").click());
    act(() => confirm());
    await harness.flush();
    expect(api.unsubscribe).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain("请先点「重新读取」");
    unmount();
    const next = mount({ onChanged: changed });
    await harness.flush();
    input(next.host.querySelector<HTMLInputElement>("#subscribe-folder")!, "再次创建");
    act(() => button(next.host, "新建").click());
    await harness.flush();
    expect(api.createFolder).toHaveBeenCalledTimes(0);
    expect(next.host.textContent).toContain("请先点「重新读取」");
    const fresh = structuredClone(PREVIEW_FEEDS);
    const removed = fresh.uncategorized.shift()!;
    vi.mocked(api.feeds).mockResolvedValueOnce(fresh);
    act(() => button(next.host, "重新读取").click());
    await harness.flush();
    expect(next.host.textContent).not.toContain(removed.title);
    expect(next.host.querySelectorAll('[role=alert]')).toHaveLength(0);
    expect(changed).toHaveBeenLastCalledWith(fresh);
    expect(api.unsubscribe).toHaveBeenCalledTimes(1);
  });

});
