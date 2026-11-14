import { expect, it } from "vitest";
import type { FeedTree, FeedView } from "../bindings";
import { createApiStub } from "./api";
import { applyFeedTreeOrder } from "./treeOrder";

function feed(id: number, title: string, folder_id: number, position: number): FeedView {
  return { id, title, icon_href: "", url: "", error: null, noisy: false, folder_id, retention_days: null, health: null, position, empty: false };
}

function sample(): FeedTree {
  return {
    folders: [
      { id: 10, name: "技术", label: "技术", indent: 0, position: 1, feeds: [feed(11, "Rust", 10, 0), feed(12, "CSS", 10, 1)] },
      { id: 20, name: "新闻", label: "新闻", indent: 0, position: 2, feeds: [feed(21, "晚报", 20, 0)] },
    ],
    uncategorized: [feed(30, "随笔", 0, 0)],
    folder_combo: ["未分类"],
  };
}

const order = (tree: FeedTree) => [
  ...tree.uncategorized.map((item) => `feed:${item.id}@${item.position}`),
  ...tree.folders.map((folder) => `folder:${folder.id}@${folder.position}:${folder.feeds.map((item) => item.id).join(",")}`),
];

it("renumbers both parents when a feed moves to the root front, and restore writes that layout back", () => {
  const moved = applyFeedTreeOrder(sample(), "move", { items: ["feed:11"], parentFolderId: null, before: "feed:30" });
  expect(moved.tree.uncategorized.map((item) => item.id)).toEqual([11, 30]);
  expect(moved.tree.folders.find((folder) => folder.id === 10)?.feeds.map((item) => item.id)).toEqual([12]);
  expect(moved.layout).toEqual(expect.arrayContaining([
    { item: "feed:11", parent_folder_id: 10, position: 0 },
    { item: "feed:30", parent_folder_id: null, position: 0 },
  ]));
  const restored = applyFeedTreeOrder(moved.tree, "restore", { layout: moved.layout });
  expect(order(restored.tree)).toEqual(order(sample()));
});

it("appends a feed inside a folder", () => {
  const moved = applyFeedTreeOrder(sample(), "move", { items: ["feed:30"], parentFolderId: 20, before: null });
  expect(moved.tree.folders.find((folder) => folder.id === 20)?.feeds.map((item) => item.id)).toEqual([21, 30]);
  expect(moved.tree.uncategorized).toEqual([]);
});

it("skips deleted rows when restoring and keeps a plain-javascript source", () => {
  const layout = applyFeedTreeOrder(sample(), "move", { items: ["feed:30"], parentFolderId: 20, before: null }).layout;
  const gone = applyFeedTreeOrder(sample(), "restore", { layout: [...layout, { item: "feed:99", parent_folder_id: null, position: 4 }] });
  expect(gone.tree.uncategorized.map((item) => item.id)).toEqual([30]);
  const again = eval(`(${applyFeedTreeOrder.toString()})`) as typeof applyFeedTreeOrder;
  const moved = again(sample(), "move", { items: ["feed:21"], parentFolderId: null, before: "folder:10" });
  expect(moved.tree.uncategorized.map((item) => item.id)).toEqual([30, 21]);
});

it("mutates the api stub tree with the same rules", async () => {
  const stub = createApiStub();
  stub.seedFeeds(sample());
  const layout = await stub.api.moveTreeItems(["feed:30"], 20, null);
  expect(stub.readFeeds().folders.find((folder) => folder.id === 20)?.feeds.map((item) => item.id)).toEqual([21, 30]);
  await stub.api.restoreTreeLayout(layout);
  expect(stub.readFeeds().uncategorized.map((item) => item.id)).toEqual([30]);
});
