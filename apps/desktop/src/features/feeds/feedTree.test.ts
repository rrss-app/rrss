import { expect, it } from "vitest";
import type { FeedTree, FeedView } from "../../lib/api";
import { folderMoveTargets, planDrop, planShift, planToTop, treeRows, type TreeRow } from "./feedTree";

function feed(id: number, title: string, folder_id: number, position: number): FeedView {
  return { id, title, icon_href: "", url: `https://example.test/${id}`, error: null, noisy: false, folder_id, retention_days: null, health: null, position, empty: false };
}

const tree: FeedTree = {
  folders: [
    { id: 10, name: "技术", label: "技术", indent: 0, position: 0, feeds: [feed(11, "Rust", 10, 0)] },
    { id: 12, name: "前端", label: "技术 / 前端", indent: 1, position: 1, feeds: [] },
    { id: 20, name: "新闻", label: "新闻", indent: 0, position: 0, feeds: [feed(21, "晚报", 20, 2)] },
  ],
  uncategorized: [feed(30, "随笔", 0, 0), feed(31, "早报", 0, 5)],
  folder_combo: [],
};

const keys = (open: number[] = [], filter = "") => treeRows(tree, null, new Set(open), filter).map((row) => row.key);

it("mixes folders and feeds by position, with folders first on a tie", () => {
  // 根层 position 0：文件夹 10（id 10）先于文件夹 20（id 20），再是源 30；源 31 的 position 更大。
  expect(keys()).toEqual(["all", "folder:10", "folder:20", "feed:30", "feed:31"]);
  expect(keys([10])).toEqual(["all", "folder:10", "feed:11", "folder:12", "folder:20", "feed:30", "feed:31"]);
});

it("hides an empty manual feed", () => {
  const hidden = feed(40, "手动收录", 0, 9);
  hidden.url = "rrss:manual";
  hidden.empty = true;
  const withManual = { ...tree, uncategorized: [...tree.uncategorized, hidden] };
  expect(treeRows(withManual, null, new Set(), "").some((row) => row.key === "feed:40")).toBe(false);
});

it("keeps a matching folder subtree and drops 全部订阅 while filtering", () => {
  expect(keys([], "技术")).toEqual(["folder:10", "feed:11", "folder:12"]);
  expect(keys([], "晚报")).toEqual(["folder:20", "feed:21"]);
});

it("plans sibling moves and refuses a folder dropped into its descendant", () => {
  expect(planShift(tree, "feed:30", -1)?.before).toBe("folder:20");
  expect(planShift(tree, "feed:30", 1)?.before).toBeNull();
  expect(planToTop(tree, "feed:31")?.before).toBe("folder:10");
  expect(planToTop(tree, "folder:10")).toBeNull();
  const news = row("folder:20");
  expect(planDrop(tree, "feed:30", news, "into")).toMatchObject({ parentFolderId: 20, before: null, expand: 20 });
  expect(planDrop(tree, "folder:10", row("folder:12"), "into")).toBeNull();
  expect(planDrop(tree, "folder:10", row("feed:11"), "before")).toBeNull();
  expect(folderMoveTargets(tree, "folder:10").map((folder) => folder.id)).toEqual([20]);
  expect(folderMoveTargets(tree, "feed:30").map((folder) => folder.depth)).toEqual([0, 1, 0]);
});

function row(key: string): TreeRow {
  const found = treeRows(tree, null, new Set([10, 12, 20]), "").find((item) => item.key === key);
  if (!found) throw new Error(key);
  return found;
}
