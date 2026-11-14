import { execFileSync } from "node:child_process";
import type { ArticleView, ChatMessageView, ClusterArticleView, ClusterSummaryView, DigestSourceView, DigestView, FeedTree, FeedView, ReaderView, RulesPage } from "../src/bindings";
import { threadFixture } from "../src/test/fixtures/chat";
import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_FOCUS, MOCK_RULES, settingsTab, type MockHostOptions } from "./mockHost";

// README 截图：只在更新 README 时运行，CI 与日常 e2e 跳过。
//   RRSS_README_SHOTS=1 pnpm --dir apps/desktop test:e2e e2e/readme-shots.spec.ts
// 数据是演示用的模拟宿主内容，不调用模型、不碰日常库。PNG 先写到 test-results/readme-shots/，
// 再用 Python Pillow 转成无损 WebP 写到 doc/images/（体积约为 PNG 的四成）。
test.skip(!process.env.RRSS_README_SHOTS, "只在更新 README 截图时运行");
test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });

const PNG_DIR = "../../test-results/readme-shots";
const OUT = "../../doc/images";
const TO_WEBP = "import sys; from PIL import Image; Image.open(sys.argv[1]).save(sys.argv[2], 'WEBP', lossless=True, method=6)";
const MINUTE = 60_000;
const now = Date.now();
const ago = (minutes: number) => now - minutes * MINUTE;
const yesterdayAt = (hours: number, minutes: number) => {
  const at = new Date();
  at.setDate(at.getDate() - 1);
  at.setHours(hours, minutes, 0, 0);
  return at.getTime();
};
const today = new Date();
const todayLabel = `${today.getMonth() + 1}月${today.getDate()}日`;

// ── 订阅 ────────────────────────────────────────────────
const feed = (id: number, title: string, folder_id: number, url: string, extra: Partial<FeedView> = {}): FeedView => ({
  id, title, url, folder_id, icon_href: "", error: null, noisy: false, retention_days: null,
  health: null, position: id, empty: false, ...extra,
});

const FEEDS: FeedTree = {
  folders: [
    { id: 1, name: "技术", label: "技术", indent: 0, position: 0, feeds: [
      feed(1, "Byte Harbor", 1, "https://byte-harbor.example/rss"),
      feed(2, "Kernel Ledger", 1, "https://kernel-ledger.example/rss"),
      feed(3, "Compiler Notes", 1, "https://compiler-notes.example/feed.xml"),
      feed(4, "Layout & Type", 1, "https://layout-type.example/feed"),
    ] },
    { id: 2, name: "中文", label: "中文", indent: 0, position: 1, feeds: [
      feed(5, "纸上谈码", 2, "https://zhishang.example/feed"),
      feed(6, "慢读札记", 2, "https://mandu.example/atom.xml"),
      feed(7, "开源前哨", 2, "https://kaiyuan.example/rss"),
      feed(8, "闲聊广场", 2, "https://xianliao.example/index.xml", { noisy: true, health: { code: "feed_health_noisy", params: { window_days: 14 } } }),
    ] },
    { id: 3, name: "科技新闻", label: "科技新闻", indent: 0, position: 2, feeds: [
      feed(9, "Circuit Review", 3, "https://circuit-review.example/feed"),
      feed(10, "Penguin Bench", 3, "https://penguin-bench.example/rss"),
    ] },
  ],
  uncategorized: [feed(11, "Local-first Lab", 0, "https://localfirst-lab.example/index.xml")],
  folder_combo: ["技术", "中文", "科技新闻"],
};
const feedTitle = (id: number) => [...FEEDS.folders.flatMap(f => f.feeds), ...FEEDS.uncategorized].find(f => f.id === id)?.title ?? "";

// ── 今日焦点 ────────────────────────────────────────────
const article = (id: number, title: string, feed_id: number, published_at: number, extra: Partial<ArticleView> = {}): ArticleView => ({
  id, title, feed_id, feed_title: feedTitle(feed_id), feed_icon_href: "", published_at, url: `https://example.org/${id}`,
  read: false, starred: false, score: 0, summary: "", row_kind: 0, cluster_id: 0, cluster_size: 1, cluster_label: "",
  cluster_span: "", interest_reason: null, rank_reasons: [], noisy: false, spot_check: false, ...extra,
});

const LOCAL_FIRST = article(101, "本地优先软件的第二个十年：同步引擎、CRDT 与单文件数据库", 1, ago(18), {
  score: 14, interest_reason: { code: "legacy_text", params: { text: " · 本地优先", prefix: null } },
  summary: "五个本地优先应用的架构复盘：数据先落本机 SQLite，同步只是后台的一层。离线可用、可迁移，服务关停也不丢数据。",
});
const WAYLAND_REP = article(901, "Wayland 成为默认会话之后：X11 会话进入维护模式", 2, ago(42), {
  score: 8, cluster_id: 900, cluster_size: 3,
  summary: "多个发行版的新版本不再默认安装 X11 会话；XWayland 继续承担旧应用的兼容。",
});
const WAYLAND_OLD = article(902, "X.Org 会话在新版发行版中改为可选组件", 10, ago(5 * 60), { cluster_id: 900, cluster_size: 3, read: true });
const WAYLAND_NEW = article(903, "国产发行版跟进 Wayland：输入法与远程桌面是最后两块短板", 7, ago(95), { cluster_id: 900, cluster_size: 3 });
const WAYLAND_HEAD = article(900, "主流发行版默认会话全面转向 Wayland", 0, 0, {
  feed_title: "", published_at: null, row_kind: 1, cluster_id: 900, cluster_size: 3,
  cluster_label: "主流发行版默认会话全面转向 Wayland", cluster_span: "昨天 → 今天",
  cluster_sources: [{ feed_id: 2, title: "Kernel Ledger", icon_href: "" }, { feed_id: 10, title: "Penguin Bench", icon_href: "" }, { feed_id: 7, title: "开源前哨", icon_href: "" }],
});

const FOCUS_ARTICLES: ArticleView[] = [
  LOCAL_FIRST,
  WAYLAND_HEAD,
  WAYLAND_REP,
  article(102, "一个人维护 RSS 阅读器十年：我为什么坚持不做算法推荐", 5, ago(2 * 60 + 10), {
    score: 6, starred: true,
    summary: "规则比黑盒更可控：关键词、作者与来源三类过滤覆盖了九成需求，剩下的交给手动收藏。",
  }),
  article(103, "SQLite 为什么适合做应用的文件格式", 6, ago(3 * 60 + 25), {
    score: 5, interest_reason: { code: "legacy_text", params: { text: " · SQLite", prefix: null } },
    summary: "单文件、带事务、可查询，比自定义二进制格式或一堆 JSON 更耐用，也更容易备份与迁移。",
  }),
  article(104, "长文阅读体验：行宽、字号与段落节奏", 4, ago(5 * 60 + 40), {
    score: 3, read: true,
    summary: "中文正文每行 30–40 字最舒适；行高与段距一起调，比单独加大字号更有效。",
  }),
  article(105, "大家现在都用什么方式管理信息流？", 8, yesterdayAt(21, 40), {
    score: -3, noisy: true, summary: "从 RSS、Newsletter 到稍后读，讨论了两百多楼。",
  }),
  article(106, "在浏览器里跑小模型：WebGPU 推理的现状与限制", 9, yesterdayAt(9, 12), {
    score: 4, summary: "量化后的小模型已经能在笔记本浏览器里流畅对话，瓶颈在显存与首次加载。",
  }),
];

const FOCUS = {
  ...MOCK_FOCUS,
  digest_banner: `今日简报 · ${todayLabel}`,
  digest_lede: "本地优先软件回潮，Linux 桌面全面转向 Wayland，浏览器端推理走向日常。",
  articles: FOCUS_ARTICLES,
  has_more: false,
};

const clusterRef = (row: ArticleView, one_liner = ""): ClusterArticleView => ({
  article_id: row.id, feed_id: row.feed_id, feed_title: row.feed_title, icon_href: "", title: row.title, published_at: row.published_at, one_liner,
});
const WAYLAND_SUMMARY: ClusterSummaryView = {
  cluster_id: 900, state: "ready", model_ready: true, estimate_tokens: 2000, basis: 3, generated_at: ago(6), fresh: [],
  members: [clusterRef(WAYLAND_OLD, "X.Org 会话改为可选组件"), clusterRef(WAYLAND_REP, "X11 会话进入维护模式"), clusterRef(WAYLAND_NEW)],
  overview: "**多个主流发行版**在新版本中把 Wayland 设为唯一默认会话，X11 会话改为可选安装；XWayland 继续兼容旧应用。",
  additions: [{ text: "国内发行版的进度取决于输入法框架与远程桌面协议的适配。", sources: [clusterRef(WAYLAND_NEW)] }],
  conflicts: [{ text: "X11 会话还会维护多久说法不一：一说两年，一说跟随上游。", sources: [clusterRef(WAYLAND_OLD), clusterRef(WAYLAND_REP)] }],
};

// ── 阅读器 ──────────────────────────────────────────────
const t = (text: string) => ({ t: "t", text });
const READER: ReaderView = {
  id: LOCAL_FIRST.id, source_hash: "readme-local-first", title: LOCAL_FIRST.title,
  meta: "Byte Harbor · 18 分钟前 · 约 8 分钟", feed_icon_href: "", url: "https://example.org/local-first",
  summary: "作者复盘了五个本地优先应用：数据先写进本机单文件数据库，同步引擎只在后台合并变更。结论是离线体验与数据可迁移性带来的收益，远大于同步实现的复杂度。",
  score: 14, feedback: 0, starred: false,
  reasons: [
    { rule_id: 11, dimension: "title_kw", snippet: "本地优先", code: "legacy_text", params: { text: "标题含 本地优先", prefix: null }, weight: 10 },
    { rule_id: 12, dimension: "body_phrase", snippet: "CRDT", code: "legacy_text", params: { text: "正文含 CRDT", prefix: null }, weight: 2 },
    { rule_id: 0, dimension: "interest", snippet: "", code: "legacy_text", params: { text: "兴趣 · 本地优先", prefix: null }, weight: 2 },
  ],
  media: [], embed_covers: [], translated_title: "", translated_document: null, translation_ready: false, translation_note: "",
  truncated: false, pruned: false, summary_stale: false,
  document: {
    v: 1,
    blocks: [
      { t: "p", inlines: [t("十年前，「本地优先」还是一篇论文里的设想：软件应该像纸笔一样，数据始终在你手里，网络只是锦上添花。今天，越来越多的应用把这句话当成了架构的起点。")] },
      { t: "h", level: 2, inlines: [t("数据先落在本机")] },
      { t: "p", inlines: [t("五个应用的共同点是：所有写入先进本机的 "), { t: "c", text: "SQLite" }, t(" 文件，界面只读本地状态；同步引擎在后台把变更合并到其他设备。断网时一切照常，只是暂时看不到别处的修改。")] },
      { t: "q", inlines: [t("离线不是一种故障模式，而是默认状态。")] },
      { t: "h", level: 2, inlines: [t("同步只是一层")] },
      { t: "li", indent: 0, marker: "•", inlines: [{ t: "s", children: [t("CRDT")] }, t("：适合文本与列表，合并无冲突，但元数据会持续增长。")] },
      { t: "li", indent: 0, marker: "•", inlines: [{ t: "s", children: [t("操作日志")] }, t("：服务端排序，实现简单，需要在线才能确定最终顺序。")] },
      { t: "li", indent: 0, marker: "•", inlines: [{ t: "s", children: [t("整库快照")] }, t("：最朴素也最可靠，适合单人多设备。")] },
      { t: "pre", lang: "sql", text: "PRAGMA journal_mode = WAL;\nCREATE TABLE changes (id INTEGER PRIMARY KEY, site TEXT, clock INTEGER, op BLOB);" },
      { t: "p", inlines: [t("作者的结论是：先把单机体验做到极致，再决定要哪一种同步。多数个人工具甚至不需要实时协作。")] },
    ],
  },
};

// 其余文章的正文：搜索命中各自的段落。
const brief = (row: ArticleView, paragraphs: string[]): ReaderView => ({
  ...READER, id: row.id, source_hash: `readme-${row.id}`, title: row.title, url: row.url, summary: row.summary, score: row.score,
  meta: `${row.feed_title} · 约 5 分钟`, reasons: [], starred: row.starred,
  document: { v: 1, blocks: paragraphs.map(text => ({ t: "p", inlines: [t(text)] })) },
});
const READERS: Record<number, ReaderView> = Object.fromEntries([
  brief(WAYLAND_REP, ["新一轮发行版更新里，Wayland 会话成了唯一的默认选项。X11 会话仍可从软件源安装，但不再随桌面一起预装。", "XWayland 继续负责旧应用的兼容，大部分用户不会察觉到差别。"]),
  brief(FOCUS_ARTICLES[3], ["十年前我写下第一行代码时，就决定所有文章都存进一个 SQLite 文件：备份就是复制，迁移就是拷走。", "过滤规则只有三类：关键词、作者与来源。它们覆盖了九成需求，而且每一次隐藏都能说清原因。"]),
  brief(FOCUS_ARTICLES[4], ["应用的文件格式要同时满足可靠、可查询与可迁移。SQLite 用一个文件就做到了这三点。", "相比自定义二进制格式，它自带事务；相比一堆 JSON，它支持索引与全文检索，备份时用 VACUUM INTO 即可得到一致的快照。"]),
  brief(FOCUS_ARTICLES[6], ["楼主用了十年 RSS，最近开始把 Newsletter 也转成订阅源，想听听大家的做法。"]),
  brief(FOCUS_ARTICLES[5], ["中文正文每行 30 到 40 个字最舒适。比加大字号更有效的，是让行高与段距一起变化。"]),
  brief(FOCUS_ARTICLES[7], ["量化后的小模型已经能在笔记本浏览器里流畅对话，权重缓存在本地，第二次打开几乎不用等待。", "瓶颈主要在显存与首次加载的下载量。"]),
].map(view => [view.id, view]));

// ── 简报 ────────────────────────────────────────────────
const source = (row: ArticleView, number: number): DigestSourceView => ({
  article_id: row.id, title: row.title, feed_title: row.feed_title, number, summary: row.summary || null, published_at: row.published_at, status: "available",
});
const SOURCES = [source(LOCAL_FIRST, 1), source(FOCUS_ARTICLES[4], 2), source(WAYLAND_REP, 3), source(WAYLAND_NEW, 4), source(FOCUS_ARTICLES[7], 5)];
const DIGEST: DigestView = {
  id: 101, period: "daily", period_label: "日报", period_start: new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime(),
  date_label: todayLabel, revision: "readme-digest", source_state: "verified", source_note: "", source_index: SOURCES, unlocated_sources: [],
  body: "",
  items: [
    { block_id: 1, sources: [SOURCES[0], SOURCES[1]] },
    { block_id: 2, sources: [SOURCES[2], SOURCES[3]] },
    { block_id: 3, sources: [SOURCES[4]] },
  ],
  document: {
    v: 1,
    blocks: [
      { t: "h", level: 3, inlines: [t("今日三条主线")] },
      { t: "li", marker: "1.", indent: 0, inlines: [{ t: "s", children: [t("本地优先回潮")] }, t("：数据先落本机单文件数据库，同步退到后台；SQLite 被越来越多地当作应用文件格式。")] },
      { t: "li", marker: "2.", indent: 0, inlines: [{ t: "s", children: [t("Wayland 成为默认")] }, t("：主流发行版把 X11 会话改为可选，国内发行版卡在输入法与远程桌面。")] },
      { t: "li", marker: "3.", indent: 0, inlines: [{ t: "s", children: [t("浏览器端推理")] }, t("：量化小模型已能在笔记本浏览器里流畅运行，瓶颈转向显存与首次加载。")] },
      { t: "p", inlines: [t("共同的方向是「把计算和数据留在用户自己的机器上」。")] },
    ],
  },
};
const PREVIOUS_DIGEST: DigestView = { ...DIGEST, id: 100, date_label: "前一日", revision: "readme-digest-prev", period_start: DIGEST.period_start - 86_400_000 };

// ── 对话 ────────────────────────────────────────────────
const message = (id: number, role: string, text: string, extra: Partial<ChatMessageView> = {}): ChatMessageView => ({
  id, role, text, streaming: false, cite_article_id: 0, cite_title: "", citations: [], document: null, ...extra,
});
const THREADS = [
  threadFixture({ id: 7, title: "本地优先应用该选哪种同步方案", updated_at: ago(3) }),
  threadFixture({ id: 8, title: "对比三篇 Wayland 报道的分歧", updated_at: ago(60) }),
  threadFixture({ id: 9, title: "整理本周值得精读的长文", updated_at: yesterdayAt(22, 5) }),
];
const MESSAGES: Record<number, ChatMessageView[]> = {
  7: [
    message(1, "user", "我想给个人笔记工具加多设备同步，这篇里提到的三种方案该怎么选？", {
      citations: [
        { kind: "article", article_id: LOCAL_FIRST.id, title: LOCAL_FIRST.title, feed_title: "Byte Harbor", url: LOCAL_FIRST.url, excerpt: "", block_id: 0 },
        { kind: "excerpt", article_id: LOCAL_FIRST.id, title: LOCAL_FIRST.title, feed_title: "Byte Harbor", url: LOCAL_FIRST.url, excerpt: "整库快照：最朴素也最可靠，适合单人多设备。", block_id: 7 },
      ],
    }),
    message(2, "assistant", "单人多设备，先用整库快照。", {
      document: {
        v: 1,
        blocks: [
          { t: "p", inlines: [t("如果只是你一个人在几台设备之间用，"), { t: "s", children: [t("先选整库快照")] }, t("，理由有三：")] },
          { t: "li", indent: 0, marker: "1.", inlines: [{ t: "s", children: [t("冲突少")] }, t("：同一时间通常只在一台设备上编辑，按修改时间取新即可。")] },
          { t: "li", indent: 0, marker: "2.", inlines: [{ t: "s", children: [t("实现小")] }, t("：备份与同步是同一份文件，可以直接复用 "), { t: "c", text: "VACUUM INTO" }, t(" 生成快照。")] },
          { t: "li", indent: 0, marker: "3.", inlines: [{ t: "s", children: [t("可迁移")] }, t("：快照就是普通 SQLite 文件，换工具也带得走。")] },
          { t: "p", inlines: [t("等需要多人实时协作时再引入 CRDT。原文对元数据增长的提醒值得先看一遍，见 "), { t: "a", href: `rrss://article/${LOCAL_FIRST.id}`, children: [t("同步只是一层")] }, t("；"), { t: "a", href: "rrss://article/103", children: [t("SQLite 作为文件格式")] }, t(" 一文也给了快照的具体做法。")] },
        ],
      },
    }),
    message(5, "user", "库变大以后，每次传整个文件会不会太慢？"),
    message(6, "assistant", "几十 MB 以内不用担心。", {
      document: {
        v: 1,
        blocks: [
          { t: "p", inlines: [t("个人笔记库通常只有几十 MB，压缩后整库上传只要几秒。真到了几百 MB，再按表记录修改时间、只传变化的行，同步逻辑仍然留在本机。")] },
        ],
      },
    }),
  ],
  8: [message(3, "user", "三篇报道对 X11 维护期限的说法为什么不一样？")],
  9: [message(4, "user", "从这周收藏里挑三篇适合周末精读的长文。")],
  301: [
    message(11, "user", "展开第二条：国内发行版具体卡在哪里？"),
    message(12, "assistant", "主要是输入法与远程桌面。", {
      document: { v: 1, blocks: [
        { t: "p", inlines: [t("据 "), { t: "a", href: "rrss://article/903", children: [t("开源前哨的报道")] }, t("，卡点集中在两处：输入法框架要改走 Wayland 的文本输入协议，候选框定位仍有问题；远程桌面要从截屏转发换成门户接口，企业里常用的方案还没跟上。")] },
      ] },
    }),
  ],
};

// ── 订阅时间线 ──────────────────────────────────────────
const TIMELINE: ArticleView[] = [
  LOCAL_FIRST,
  WAYLAND_REP,
  WAYLAND_NEW,
  FOCUS_ARTICLES[3],
  FOCUS_ARTICLES[4],
  article(107, "终端里的富文本：现代终端模拟器支持了哪些转义序列", 1, ago(4 * 60)),
  WAYLAND_OLD,
  FOCUS_ARTICLES[5],
  article(108, "编译器性能：下一阶段的工作计划", 3, yesterdayAt(23, 5), { read: true }),
  FOCUS_ARTICLES[7],
  article(109, "把 Newsletter 转成 RSS 的几种方法", 5, yesterdayAt(8, 30), { read: true }),
];

const RULES: RulesPage = {
  ...MOCK_RULES,
  rules: [
    { ...MOCK_RULES.rules[0], id: 11, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", pattern: "本地优先" },
    { ...MOCK_RULES.rules[1], id: 13, summary: { code: "rule_summary", params: { effect: "hide" } }, scope_name: "" },
    { ...MOCK_RULES.rules[2], id: 14, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "Byte Harbor", scope_id: 1 },
    { ...MOCK_RULES.rules[0], id: 12, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", dimension_id: "body_phrase", pattern: "CRDT", weight: 2 },
    { ...MOCK_RULES.rules[3], id: 15, summary: { code: "rule_summary", params: { effect: "hide" } }, scope_name: "科技新闻", scope_id: 3, enabled: true },
  ],
  folders: FEEDS.folders.map(f => ({ id: f.id, label: f.label })),
  feeds: FEEDS.folders.flatMap(f => f.feeds).map(f => ({ id: f.id, label: f.title })),
};

const HOST: MockHostOptions = {
  focus: FOCUS, feeds: FEEDS, threads: THREADS, messages: MESSAGES, digests: [DIGEST, PREVIOUS_DIGEST],
  reader: READER, readers: READERS, clusterArticles: { 900: [WAYLAND_OLD, WAYLAND_REP, WAYLAND_NEW] }, clusterSummaries: { 900: WAYLAND_SUMMARY },
  timelineArticles: TIMELINE, rules: RULES,
};

async function open(page: Page, theme: "light" | "dark" = "light") {
  await page.emulateMedia({ reducedMotion: "reduce", colorScheme: theme });
  await installMockHost(page, HOST);
  await page.goto("/");
  await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
  await expect(page.getByRole("article").first()).toBeVisible();
}

async function save(page: Page, name: string) {
  // 左缘是阅读器篇目抽屉的热区，鼠标放到右下角，避免唤出抽屉或悬停态。
  const { width, height } = page.viewportSize()!;
  await page.mouse.move(width - 1, height - 1);
  await page.waitForTimeout(150);
  const png = `${PNG_DIR}/${name}.png`;
  await page.screenshot({ path: png, animations: "disabled" });
  execFileSync("python3", ["-c", TO_WEBP, png, `${OUT}/${name}.webp`]);
}

for (const theme of ["light", "dark"] as const) {
  test(`focus ${theme}`, async ({ page }) => {
    await open(page, theme);
    await save(page, `focus-${theme}`);
  });
}

test("cluster summary", async ({ page }) => {
  await open(page);
  const focus = page.locator(".focus");
  await focus.locator(".cluster-head button.cluster").click();
  await expect(focus.locator(".cluster-overview")).toContainText("多个主流发行版");
  // 组面板上沿放到工具栏下方，上一张卡片整个滚出视口，不留半行标题。
  await focus.locator(".cluster-panel").evaluate(el => {
    let scroller = el.parentElement;
    while (scroller && !/(auto|scroll)/.test(getComputedStyle(scroller).overflowY)) scroller = scroller.parentElement;
    (scroller ?? document.scrollingElement!).scrollBy(0, el.getBoundingClientRect().top - 72);
  });
  await save(page, "cluster");
});

test("reader", async ({ page }) => {
  await open(page);
  await page.getByRole("article").first().click();
  await expect(page.locator(".drawer.reader .reader-summary")).toBeVisible();
  await save(page, "reader");
});

test("digest", async ({ page }) => {
  await open(page, "dark");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.locator(".digest-date")).toHaveText(todayLabel);
  await save(page, "digest");
});

test("chat", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: THREADS[0].title }).click();
  await expect(page.locator(".bubble.assistant").last()).toBeVisible();
  await save(page, "chat");
});

test("feeds", async ({ page }) => {
  await open(page);
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await expect(page.getByRole("tree", { name: "订阅" })).toBeVisible();
  for (const id of [1, 2]) await page.locator(`.feeds-row[data-key="folder:${id}"] .feeds-twist`).click();
  await save(page, "feeds");
});

test("search", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "搜索文章" }).click();
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("SQLite");
  await expect(search.getByRole("option").first()).toBeVisible();
  await save(page, "search");
});

test("rules", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "rules").click();
  await expect(settings.getByText("建议规则")).toBeVisible();
  await save(page, "rules");
});
