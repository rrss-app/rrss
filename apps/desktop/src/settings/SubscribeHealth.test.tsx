import { act } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SubscribePane } from "./SubscribePane";
import { api, type FeedTree } from "../lib/api";
import { PREVIEW_FEEDS } from "../lib/preview";
import { createReactHarness } from "../test/react";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return { ...actual, api: { ...actual.api, feeds: vi.fn(), saveFeed: vi.fn(), unsubscribeFeeds: vi.fn(), dismissFeedHealth: vi.fn() } };
});
const harness = createReactHarness();
let client: QueryClient;
const button = (host: HTMLElement, text: string) => [...host.querySelectorAll("button")].find((b) => b.textContent === text)!;
const row = (host: HTMLElement, title: string) => [...host.querySelectorAll<HTMLElement>(".feed-row")].find((r) => r.textContent?.includes(title))!;

/** 预览订阅树上加两种健康状态：一个长期失败、一个长期未更新。 */
function treeWithHealth(): FeedTree {
  const tree = structuredClone(PREVIEW_FEEDS);
  const all = [...tree.uncategorized, ...tree.folders.flatMap((f) => f.feeds)];
  Object.assign(all[0], { health: { code: "feed_health_stale", params: { quiet_days: 47, avg_interval_days: 6, failed: false, last_article_at: null, last_success_at: null } } });
  Object.assign(all[2], { health: { code: "feed_health_failing", params: { fail_count: 12, failing_days: 9, last_success_at: null } } });
  return tree;
}

describe("SubscribePane feed health", () => {
  beforeEach(() => {
    client = new QueryClient();
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    notifyManager.setScheduler(queueMicrotask);
    vi.mocked(api.feeds).mockResolvedValue(treeWithHealth());
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    notifyManager.setScheduler((callback) => setTimeout(callback, 0));
    vi.resetAllMocks();
    vi.unstubAllGlobals();
  });

  it("saves a per-feed retention choice", async () => {
    vi.mocked(api.feeds).mockResolvedValue(structuredClone(PREVIEW_FEEDS));
    const { host } = harness.mount(<QueryClientProvider client={client}><SubscribePane onAsk={() => {}} /></QueryClientProvider>);
    await harness.flush();
    act(() => button(row(host, "Compiler Notes"), "编辑").click());
    act(() => host.querySelector<HTMLButtonElement>('button[aria-label="保留文章正文"]')!.click());
    const custom = [...document.querySelectorAll<HTMLButtonElement>('[role="menu"] [role="menuitemradio"]')].find((item) => item.textContent === "自定义天数")!;
    act(() => custom.click());
    const days = host.querySelector<HTMLInputElement>('input[type="number"]')!;
    expect(days.value).toBe("90");
    vi.mocked(api.saveFeed).mockResolvedValue(structuredClone(PREVIEW_FEEDS));
    act(() => button(host, "保存修改").click());
    await harness.flush();
    expect(api.saveFeed).toHaveBeenCalledWith(expect.objectContaining({ retention: "90", title: null, url: null, folder_id: null }));
  });
});
