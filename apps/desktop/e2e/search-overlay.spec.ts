import { expect, test } from "./fixtures";
import { shot } from "./shots";
import { installMockHost, MOCK_FOCUS, MOCK_MESSAGES, MOCK_READER } from "./mockHost";

const lastQuery = (page: import("@playwright/test").Page) =>
  page.evaluate(() => (window as any).__mockHost.args.search_articles?.query);

test("search overlay filters by scope, source and time, keeps recent searches and explains empty results", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  const input = search.getByRole("searchbox");
  await input.fill("sqlite");
  await expect(search.getByRole("option").first()).toBeVisible();
  await expect.poll(() => lastQuery(page)).toMatchObject({ query: "sqlite", scope: "all", feed_ids: [], published_since: null });

  // 范围：只剩未读，结果行不带「已读」。
  await search.getByRole("button", { name: "范围", exact: true }).click();
  await page.getByRole("menuitemradio", { name: "未读" }).click();
  await expect.poll(() => lastQuery(page)).toMatchObject({ scope: "unread" });
  await expect(search.getByRole("option").filter({ hasText: "已读" })).toHaveCount(0);

  // 时间：选 7 天后带上发布时间下界。
  await search.getByRole("button", { name: /时间/ }).click();
  await page.getByRole("menuitemradio", { name: "7 天" }).click();
  await expect(search.getByRole("button", { name: /时间/ })).toContainText("7 天");
  const since = (await lastQuery(page)).published_since;
  expect(since).toBeGreaterThan(Date.now() - 8 * 86_400_000);
  expect(since).toBeLessThan(Date.now() - 6 * 86_400_000);

  // 来源：多选，菜单保持打开；Esc 只关菜单。
  await search.getByRole("button", { name: /来源/ }).click();
  await page.getByRole("menuitemcheckbox", { name: "Compiler Notes" }).click();
  await expect.poll(async () => (await lastQuery(page)).feed_ids).toHaveLength(1);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu", { name: "按来源筛选" })).toHaveCount(0);
  await expect(search).toBeVisible();
  await expect(search.getByRole("button", { name: /来源/ })).toContainText("Compiler Notes");

  // 无结果：说明搜的是什么，可一键清除筛选。
  await input.fill("zzzz不存在的词");
  await expect(search.getByRole("heading", { name: "没有找到「zzzz不存在的词」" })).toBeVisible();
  await search.getByRole("button", { name: "清除筛选" }).click();
  await expect.poll(() => lastQuery(page)).toMatchObject({ scope: "all", feed_ids: [], published_since: null });

  // 关闭再打开：筛选复位，最近搜索记住本次运行里有结果的词，点一下重搜。
  await input.fill("sqlite");
  await expect(search.getByRole("option").first()).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await page.keyboard.press("Control+k");
  await expect(search.getByRole("button", { name: "范围", exact: true })).toContainText("全部");
  await search.getByRole("button", { name: /^sqlite/ }).click();
  await expect(input).toHaveValue("sqlite");
  await expect(search.getByRole("option").first()).toBeVisible();
});

test("preview lists body hits by section with a position strip, folds after four and opens the clicked paragraph", async ({ page }) => {
  const p = (text: string) => ({ t: "p" as const, inlines: [{ t: "t" as const, text }] });
  const h = (text: string) => ({ t: "h" as const, level: 2, inlines: [{ t: "t" as const, text }] });
  const document = { v: 1, blocks: [
    p("开头一段，不提那个词。"),
    h("为什么要本地检索"), p("本地检索不依赖网络。"), p("检索结果和阅读位置都留在本机。"),
    h("检索的实现"), p("FTS5 负责关键词检索。"), p("向量检索放在后台作业里。"), p("中间一段，同样不提。"), p("最后再说一次检索。"),
  ] };
  await installMockHost(page, { readers: { 1: { ...MOCK_READER, document } } });
  await page.setViewportSize({ width: 1920, height: 1000 });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("检索");
  await search.getByRole("option").filter({ hasText: MOCK_READER.title }).hover();

  const preview = search.getByRole("article", { name: "预览" });
  const hits = preview.getByRole("region", { name: "正文命中" });
  await expect(hits).toContainText("正文命中 7 处");
  await expect(hits).toContainText("全文 9 段");
  // 阅读时长按文章快照估算，与阅读页同一公式。
  await expect(preview).toContainText(/约 \d+ 分钟 · 分数/);
  // 标题也算一段：命中的标题排在自己这组上面，不再重复组名。
  await expect(hits.getByText("为什么要本地检索", { exact: true })).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.search_hit_blocks)).toEqual({ articleId: 1, query: "检索" });
  await expect(hits.getByRole("button", { name: /第 \d+ 段/ })).toHaveCount(4);
  // 宽档两栏：命中位置并在元信息行里，没有第三栏。
  await expect(preview.getByRole("group", { name: "命中位置" })).toContainText("正文 7 处");
  await expect(search.getByRole("complementary")).toHaveCount(0);

  await hits.getByRole("button", { name: "展开其余 3 处" }).click();
  await expect(hits.getByRole("button", { name: /第 \d+ 段/ })).toHaveCount(7);
  await expect(hits.getByText("检索的实现", { exact: true })).toHaveCount(1);

  // 点某一段：关闭搜索，打开这篇并定位到那一段。
  await hits.getByRole("button", { name: /第 9 段/ }).click();
  await expect(search).toHaveCount(0);
  await expect(page.locator(".reader-title")).toHaveText(MOCK_READER.title);
  await expect(page.locator('[data-block-id="8"]')).toBeInViewport();
});

const para = (text: string) => ({ t: "p" as const, inlines: [{ t: "t" as const, text }] });
const heading = (text: string) => ({ t: "h" as const, level: 2, inlines: [{ t: "t" as const, text }] });
const longDocument = { v: 1, blocks: [
  para("缓存命中就在第一段，块序号是 0。"),
  ...Array.from({ length: 14 }, (_, i) => para(`中间第 ${i + 1} 段，用来把后面的段落推到首屏以外。`.repeat(4))),
  heading("后半"),
  para("第二处缓存命中，在很靠后的位置。"),
  para("第三处：缓存也在这里。"),
] };

async function openSearch(page: import("@playwright/test").Page, term: string) {
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill(term);
  await expect(search.getByRole("option").first()).toBeVisible();
  return search;
}

test("opening a result lands on the first hit, N / Shift+N cycle hits, and Esc restores the search", async ({ page }) => {
  await installMockHost(page, { readers: { 1: { ...MOCK_READER, document: longDocument } } });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  const search = await openSearch(page, "缓存");
  const rows = search.getByRole("option");
  await expect(rows).toHaveCount(1);
  await rows.first().click();

  const stage = page.locator(".reader-stage");
  const pill = stage.getByRole("status", { name: "搜索命中" });
  // 命中在第 0 块也能定位；高亮保持，不是一闪而过。
  await expect(stage.locator('[data-block-id="0"]')).toHaveClass(/reader-search-hit/);
  await expect(pill).toHaveText(/第 1 处命中（共 3 处）/);
  await page.waitForTimeout(1500);
  await expect(stage.locator('[data-block-id="0"]')).toHaveClass(/reader-search-hit/);
  await expect(stage.locator(".return-search")).toBeVisible();

  await page.keyboard.press("n");
  await expect(pill).toHaveText(/第 2 处命中/);
  await expect(stage.locator('[data-block-id="16"]')).toHaveClass(/reader-search-hit/);
  await expect(stage.locator('[data-block-id="16"]')).toBeInViewport();
  await expect(stage.locator('[data-block-id="0"]')).not.toHaveClass(/reader-search-hit/);
  await page.keyboard.press("n");
  await page.keyboard.press("n");
  await expect(pill).toHaveText(/第 1 处命中/);
  await page.keyboard.press("Shift+N");
  await expect(pill).toHaveText(/第 3 处命中/);
  await expect(stage.locator('[data-block-id="17"]')).toBeInViewport();

  // 返回：浮层、查询、结果与选中行都在。
  await page.keyboard.press("Escape");
  await expect(stage).toHaveCount(0);
  await expect(search).toBeVisible();
  await expect(search.getByRole("searchbox")).toHaveValue("缓存");
  await expect(rows.first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await page.keyboard.press("Control+k");
  await expect(search.getByRole("searchbox")).toHaveValue("");
});

test("title-only hits open from the top, a changed body version skips the jump, and j follows search order", async ({ page }) => {
  const changed = { ...MOCK_READER, id: 1, source_hash: "changed-after-search", document: longDocument };
  await installMockHost(page, { readers: { 1: { ...MOCK_READER, document: longDocument } }, readerSequences: { 1: [changed] } });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  const stage = page.locator(".reader-stage");

  // 正文版本在搜索后变了：按阅读位置规则从头打开，不跳、不显示命中提示。
  let search = await openSearch(page, "缓存");
  await search.getByRole("option").first().click();
  await expect(stage.locator(".reader-title")).toHaveText(MOCK_READER.title);
  await expect(stage.locator(".reader-search-hit")).toHaveCount(0);
  await expect(stage.getByRole("status", { name: "搜索命中" })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);

  // 只在标题命中（这篇正文里没有「检索」）：从头打开、没有命中提示；j 按搜索结果顺序换到下一篇。
  search = await openSearch(page, "检索");
  const titles = await search.getByRole("option").locator(".command-palette-title").allTextContents();
  expect(titles[0]).toBe(MOCK_READER.title);
  expect(titles.length).toBeGreaterThan(1);
  await search.getByRole("option").first().click();
  await expect(stage.locator(".reader-title")).toHaveText(titles[0]);
  await expect(stage.locator(".drawer-body")).toHaveJSProperty("scrollTop", 0);
  await expect(stage.getByRole("status", { name: "搜索命中" })).toHaveCount(0);
  // 模拟宿主各篇共用一份阅读视图（标题相同），按打开的文章 ID 核对。
  const second = MOCK_FOCUS.articles.find((row) => row.title === titles[1])!.id;
  await page.keyboard.press("j");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.open_article)).toEqual({ id: second });
  await page.keyboard.press("Escape");
  await expect(search.getByRole("option").nth(1)).toHaveAttribute("aria-selected", "true");
});

test("search finds conversations, shows the matched message and opens the thread at it", async ({ page }) => {
  // 一条长会话：命中的消息在最早的一页里，打开后要向前翻页才载得到。
  const long = Array.from({ length: 60 }, (_, i) => ({ ...MOCK_MESSAGES[7][1], id: 1000 + i, role: i % 2 ? "assistant" : "user",
    text: i === 3 ? "用 trigram 分词时，两个字的中文词也能搜到吗？" : `第 ${i + 1} 条普通消息`, document: null }));
  await installMockHost(page, { messages: { ...MOCK_MESSAGES, 8: long } });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("trigram");
  // 文章标签上就能看到对话里有几条。
  const threads = search.getByRole("button", { name: /^对话/ });
  await expect(threads).toContainText("1");
  await threads.click();
  await expect(search.getByRole("button", { name: /时间/ })).toHaveCount(0);
  const hit = search.getByRole("option").first();
  await expect(hit).toContainText("Tauri IPC 线程模型");
  await expect(hit).toContainText("助手：");
  await expect(search.locator(".search-preview")).toContainText("命中助手的回复");
  await shot(page, "../../test-results/search-overlay-threads.png");
  await page.keyboard.press("Enter");
  await expect(search).toHaveCount(0);
  const matched = page.locator('.chat-pane [data-message-id="1003"]');
  await expect(matched).toBeInViewport();
  await expect(matched).toHaveAttribute("data-flash", "");
});
