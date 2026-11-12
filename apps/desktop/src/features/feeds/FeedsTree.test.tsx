import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { FeedTree, FeedView } from "../../lib/api";
import { createApiStub, type ApiStub } from "../../test/api";
import { createReactHarness } from "../../test/react";
import { deferred } from "../../test/deferred";
import { applyFeedTreeOrder } from "../../test/treeOrder";
import { click, pressKey, typeInto } from "../../test/fixtures/shell";
import { createSettingsSchema } from "../../settings/registry";
import { FeedsTree, type TreeNotice } from "./FeedsTree";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../../lib/api")>("../../lib/api");
  const { createApiProxy } = await import("../../test/api");
  return { ...actual, api: createApiProxy(() => box.stub) };
});

const harness = createReactHarness();
const feed = (id: number, title: string, position: number): FeedView => ({
  id, title, icon_href: "", url: "", error: "", noisy: false, folder_id: 0, retention_days: null,
  health: "", health_label: "", health_detail: "", position, empty: false,
});
const TREE: FeedTree = { folders: [], uncategorized: [feed(1, "甲源", 0), feed(2, "乙源", 1), feed(3, "丙源", 2)], folder_combo: ["未分类"] };

beforeEach(() => {
  box.stub = createApiStub();
  box.stub.handle("settingsSchema", () => createSettingsSchema());
  box.stub.seedFeeds(TREE);
});
afterEach(() => {
  harness.cleanup();
  box.stub?.assertNoUnexpectedCalls();
});

const row = (host: HTMLElement, key: string) => host.querySelector<HTMLElement>(`[role=treeitem][data-key="${key}"]`)!;
function rename(host: HTMLElement, name: string, next: string) {
  click(host.querySelector(`[aria-label="${name} 的操作"]`));
  click([...document.querySelectorAll("[role=menuitem]")].find((item) => item.textContent === "重命名"));
  const input = host.querySelector<HTMLInputElement>(`[aria-label="重命名${name}"]`)!;
  typeInto(input, next);
  pressKey("Enter", {}, input);
}

it("submits undo, later moves and repeated renames through one tree write channel", async () => {
  const stub = box.stub!;
  // 宿主命令可能在不同阻塞线程里反序完成；同一时刻只允许一条树写入在途。
  let inFlight = 0;
  let maxInFlight = 0;
  const track = async <T,>(work: () => T | Promise<T>) => {
    maxInFlight = Math.max(maxInFlight, ++inFlight);
    try { return await work(); } finally { inFlight -= 1; }
  };
  const restoring = deferred<void>();
  stub.handle("moveTreeItems", (items, parentFolderId, before) => track(() => {
    const result = applyFeedTreeOrder(stub.readFeeds(), "move", { items, parentFolderId, before });
    stub.seedFeeds(result.tree);
    return result.layout;
  }));
  stub.handle("restoreTreeLayout", (layout) => track(async () => {
    await restoring.promise;
    stub.seedFeeds(applyFeedTreeOrder(stub.readFeeds(), "restore", { layout }).tree);
  }));
  const titles: string[] = [];
  const renaming = deferred<void>();
  let saves = 0;
  stub.handle("saveFeed", (args) => track(async () => {
    if (saves++ === 0) await renaming.promise;
    titles.push(args.title!);
    return stub.readFeeds();
  }));
  const notices: TreeNotice[] = [];
  const client = new QueryClient();
  const { host } = harness.mount(<QueryClientProvider client={client}>
    <FeedsTree tree={TREE} counts={null} scope="all" onSelect={() => {}} onAddFeed={() => {}} onSettings={() => {}} notify={(notice) => notices.push(notice)} />
  </QueryClientProvider>);
  await harness.flush();

  pressKey("ArrowDown", { altKey: true }, row(host, "feed:2"));
  await harness.flush();
  act(() => notices.find((notice) => notice.onAction)!.onAction!());
  pressKey("ArrowUp", { altKey: true }, row(host, "feed:2"));
  await harness.flush();
  restoring.resolve();
  await harness.flush();
  expect(maxInFlight).toBe(1);
  expect(stub.readFeeds().uncategorized.map((item) => item.id)).toEqual([1, 2, 3]);

  rename(host, "甲源", "甲");
  await harness.flush();
  rename(host, "甲源", "甲乙");
  await harness.flush();
  renaming.resolve();
  await harness.flush();
  expect(titles).toEqual(["甲", "甲乙"]);
  expect(maxInFlight).toBe(1);
  client.clear();
});
