import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS } from "./mockHost";
import { shot } from "./shots";

// 阶段 14 · 相近报道成组与汇总（第一步，只做显示）：成组面板、代表固定组首、成员一行、组尾操作。
const now = Date.now();
const base = { ...MOCK_FOCUS.articles[0], summary: "", score: 0, read: false, starred: false, row_kind: 0, cluster_size: 1, cluster_label: "", cluster_span: "" };
const before = { ...base, id: 1, title: "组前的一篇", cluster_id: 0, published_at: now - 10 * 60_000 };
const after = { ...base, id: 2, title: "组后的一篇", cluster_id: 0, published_at: now - 90 * 60_000 };
const header = { ...base, id: 900, row_kind: 1, cluster_id: 900, cluster_size: 3, cluster_label: "芯片出口新规落地", cluster_span: "11-05 → 11-06", title: "芯片出口新规落地",
  cluster_sources: [{ feed_id: 12, title: "每日快讯", icon_href: "" }, { feed_id: 11, title: "36氪", icon_href: "" }, { feed_id: 13, title: "财新", icon_href: "" }] };
const rep = { ...base, id: 901, cluster_id: 900, feed_id: 11, feed_title: "36氪", title: "芯片出口新规落地：多家厂商回应供货安排",
  summary: "厂商表示现有订单不受影响，下一代产品的海外版本需要重新送审。", published_at: now - 20 * 60_000 };
const older = { ...base, id: 902, cluster_id: 900, feed_id: 12, feed_title: "每日快讯", title: "快讯｜出口管制新规今日生效", published_at: now - 20 * 3600_000, read: true };
const newer = { ...base, id: 903, cluster_id: 900, feed_id: 13, feed_title: "财新", title: "新规细则解读：算力密度门槛如何计算", published_at: now - 60 * 60_000 };

test("a cluster is one panel: representative stays first, members fold into rows, and the footer acts on the whole group", async ({ page }) => {
  await installMockHost(page, {
    focus: { ...MOCK_FOCUS, digest_banner: "", articles: [before, header, rep, after], has_more: false },
    // 宿主按发布时间正序给成员；界面把代表固定在组首，其余新的在上。
    clusterArticles: { 900: [older, rep, newer] },
  });
  await page.goto("/");
  const focus = page.locator(".focus");
  const toggle = focus.locator(".cluster-head button.cluster");
  await expect(focus.getByRole("button", { name: "芯片出口新规落地 · 3 篇 · 展开" })).toBeVisible();
  await expect(focus.locator(".cluster-head")).toContainText("芯片出口新规落地");
  await expect(toggle).toHaveText("展开另外 2 篇");
  // 收起时组头也写来源数、叠放头像（宿主在标题行上给出）。
  await expect(focus.locator(".cluster-head")).toContainText("3 个来源");
  await expect(focus.locator(".cluster-stack .source-avatar")).toHaveCount(3);
  await expect(focus.locator('[data-cluster="rep"] .article')).toContainText("代表报道");
  await expect(focus.locator(".cluster-panel")).toHaveCount(1);
  await shot(page, "../../test-results/focus-cluster-collapsed.png");

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(focus.locator('[data-cluster="rep"] .article h3')).toHaveText(rep.title);
  await expect(focus.locator(".cluster-member h3")).toHaveText([newer.title, older.title]);
  await expect(focus.locator(".cluster-head")).toContainText("3 个来源");
  const foot = focus.locator(".cluster-foot");
  await expect(foot).toContainText("共 3 篇");
  await shot(page, "../../test-results/focus-cluster-expanded.png");

  // 全部标为已读：一次写入，只标还没读的两篇，直接生效、不带撤销。
  await foot.getByRole("button", { name: "全部标为已读" }).click();
  await expect(page.getByText("已把这组 2 篇标为已读")).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockHost.args.mark_cluster_read)).toEqual({ clusterId: 900 });
  expect(await page.evaluate(() => (window as any).__mockHost.calls.includes("mark_read"))).toBe(false);
  await expect(focus.locator(".cluster-member.unread")).toHaveCount(0);
  await expect(focus.locator('[data-cluster="rep"] .article.unread')).toHaveCount(0);

  // 引用整组：三篇进新对话的输入区。
  await foot.getByRole("button", { name: "引用整组到对话" }).click();
  await page.getByRole("button", { name: "去对话" }).click();
  await expect(page.locator(".composer .cite-row .citation-chip")).toHaveCount(3);

  await page.getByRole("button", { name: "今日焦点" }).first().click();
  await focus.locator(".cluster-foot").getByRole("button", { name: "收起" }).click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(focus.locator(".cluster-member")).toHaveCount(0);
  await expect(focus.locator('[data-cluster="rep"] .article h3')).toHaveText(rep.title);
});

// 第三步：面板内的综述，五种状态。
const ref = (row: typeof rep, one_liner = "") => ({ article_id: row.id, feed_id: row.feed_id, feed_title: row.feed_title, icon_href: "",
  title: row.title, published_at: row.published_at, one_liner });
const members = [ref(older, "出口管制新规今日生效"), ref(rep, "多家厂商回应供货安排"), ref(newer, "")];
const ready = {
  cluster_id: 900, state: "ready", model_ready: true, estimate_tokens: 2000, basis: 3, generated_at: now - 5 * 60_000, fresh: [], members,
  overview: "**出口管制新规**公布并施行，厂商表示在手订单照常交付。",
  additions: [{ text: "给出算力密度的算法和豁免清单。", sources: [ref(newer)] }],
  conflicts: [{ text: "过渡期说法不一。", sources: [ref(newer), ref(older)] }],
};
const groupFocus = (extra: Record<string, unknown> = {}) => ({ ...MOCK_FOCUS, digest_banner: "", has_more: false, articles: [{ ...header, ...extra }, rep] });

test("汇总 in the header expands the group and generates in place, then sources open the article", async ({ page }) => {
  await installMockHost(page, { focus: groupFocus(), clusterArticles: { 900: [older, rep, newer] }, clusterGenerated: { 900: ready }, holdClusterSummary: true });
  await page.goto("/");
  const focus = page.locator(".focus");
  await focus.locator(".cluster-head").getByRole("button", { name: "汇总" }).click();
  await expect(focus.locator(".cluster-head button.cluster")).toHaveAttribute("aria-expanded", "true");
  // 生成中：骨架，标出在汇总几篇。
  const busy = focus.locator('.cluster-summary[aria-busy="true"]');
  await expect(busy).toContainText("正在汇总 3 篇报道");
  await shot(page, "../../test-results/focus-cluster-summary-loading.png");
  await page.evaluate(() => (window as any).__mockHost.releaseClusterSummary());
  const summary = focus.locator(".cluster-summary");
  await expect(summary.locator(".cluster-overview b")).toHaveText("出口管制新规");
  await expect(summary).toContainText("依据 3 篇");
  await expect(summary.locator(".cluster-notes li")).toContainText(["财新给出算力密度的算法和豁免清单。"]);
  // 来源标签里有头像的首字，只核对来源名。
  await expect(summary.locator(".cluster-conflicts .cluster-chip")).toContainText(["财新", "每日快讯"]);
  // 综述独立在组头下；代表文章收成成员行，排第一并标「代表」（2026-11-08 方案 C）。
  await expect(focus.locator(".cluster-head + .cluster-summary")).toHaveCount(1);
  await expect(focus.locator('[data-cluster="rep"] .cluster-member .cluster-member-rep')).toHaveText("代表");
  await shot(page, "../../test-results/focus-cluster-summary-ready.png");
  await summary.locator(".cluster-notes .cluster-chip").click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
});

test("an expanded group offers generation, flags new reports, and updates the summary in place", async ({ page }) => {
  const stale = { ...ready, state: "stale", basis: 2, fresh: [ref(newer)] };
  await installMockHost(page, {
    focus: groupFocus({ cluster_fresh: 1 }), clusterArticles: { 900: [older, rep, newer] },
    clusterSummaries: { 900: stale }, clusterGenerated: { 900: ready },
  });
  await page.goto("/");
  const focus = page.locator(".focus");
  await expect(focus.locator(".cluster-fresh")).toHaveText("新 1");
  await focus.locator(".cluster-head button.cluster").click();
  const bar = focus.locator(".cluster-summary-stale");
  await expect(bar).toContainText("新增 1 篇：财新");
  // 旧综述照常显示。
  await expect(focus.locator(".cluster-overview")).toContainText("出口管制新规");
  await shot(page, "../../test-results/focus-cluster-summary-stale.png");
  await bar.getByRole("button", { name: "更新综述" }).click();
  await expect(bar).toHaveCount(0);
  await expect(focus.locator(".cluster-summary")).toContainText("依据 3 篇");
  await expect(focus.locator(".cluster-fresh")).toHaveCount(0);
});

test("a group not yet summarized offers to generate; without a model it lists one-liners and points to settings", async ({ page }) => {
  await installMockHost(page, { focus: groupFocus(), clusterArticles: { 900: [{ ...older, summary: "出口管制新规今日生效" }, rep, newer] }, clusterSummaryModelReady: false });
  await page.goto("/");
  const focus = page.locator(".focus");
  // 没配模型：「汇总」不调用模型，只列各篇已有的一句话。
  await focus.locator(".cluster-head").getByRole("button", { name: "汇总" }).click();
  const fallback = focus.locator(".cluster-summary");
  await expect(fallback).toContainText("生成综述要先选摘要模型");
  // 只列有一句话、又不是代表文章的篇：代表的摘要就在上方卡片里，没一句话的已在成员行里。
  await expect(fallback.locator(".cluster-notes li")).toHaveCount(1);
  await expect(fallback.locator(".cluster-notes li")).toContainText("出口管制新规今日生效");
  expect(await page.evaluate(() => (window as any).__mockHost.calls.includes("generate_cluster_summary"))).toBe(false);
  await shot(page, "../../test-results/focus-cluster-summary-nomodel.png");
  await fallback.getByRole("button", { name: "去选模型" }).click();
  await expect(page.getByRole("dialog", { name: "设置", exact: true })).toBeVisible();
});

test("the prompt row names the model use and rough cost before generating", async ({ page }) => {
  await installMockHost(page, { focus: groupFocus(), clusterArticles: { 900: [older, rep, newer] }, clusterGenerated: { 900: ready } });
  await page.goto("/");
  const focus = page.locator(".focus");
  await focus.locator(".cluster-head button.cluster").click();
  const prompt = focus.locator(".cluster-summary-row");
  await expect(prompt).toContainText("汇总这 3 篇报道");
  await expect(prompt).toContainText("用摘要模型，约 2k token");
  expect(await page.evaluate(() => (window as any).__mockHost.calls.includes("generate_cluster_summary"))).toBe(false);
  await shot(page, "../../test-results/focus-cluster-summary-none.png");
  await prompt.getByRole("button", { name: "生成综述" }).click();
  await expect(focus.locator(".cluster-overview")).toContainText("出口管制新规");
});
