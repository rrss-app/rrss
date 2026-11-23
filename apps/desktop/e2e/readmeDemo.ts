import type { ArticleView, ChatMessageView, ClusterArticleView, ClusterSummaryView, DigestSourceView, DigestView, FeedTree, FeedView, ReaderView, RulesPage } from "../src/bindings";
import { threadFixture } from "../src/test/fixtures/chat";
import { MOCK_FOCUS, MOCK_RULES, type MockHostOptions } from "./mockHost";

// README 截图的演示数据：虚构的来源与 `.example` 域名，中英文各一份，结构相同、只换文字。
// 界面文字由语言包给出；这里只有内容（标题、摘要、正文、对话）和截图脚本要点击的界面名称。

export type DemoLanguage = "zh-CN" | "en";

type Copy = {
  /** 截图脚本按名称点击的界面元素，取自对应语言包。 */
  ui: { digest: string; feeds: RegExp; feedTree: string; search: string; searchDialog: string; settings: string; suggestedRules: string };
  folders: [string, string, string];
  /** 第二个文件夹里的四个订阅（中文版是中文来源）。 */
  folderFeeds: [string, string, string, string];
  interest: string;
  localFirst: { title: string; summary: string };
  waylandHead: string;
  waylandRep: { title: string; summary: string };
  waylandOld: string;
  waylandNew: string;
  /** 焦点里其余五篇：独立维护、SQLite、长文排版、信息流讨论、浏览器推理。 */
  others: [{ title: string; summary: string }, { title: string; summary: string }, { title: string; summary: string }, { title: string; summary: string }, { title: string; summary: string }];
  timelineOnly: [string, string, string];
  lede: string;
  cluster: { overview: string; overviewCheck: string; oldLine: string; repLine: string; addition: string; conflict: string };
  reader: {
    summary: string; intro: string; h1: string; p1a: string; p1b: string; quote: string; h2: string;
    items: [[string, string], [string, string], [string, string]]; outro: string;
  };
  /** 搜索与列表点开时的正文：Wayland、独立维护、SQLite、信息流讨论、长文排版、浏览器推理。 */
  briefs: [string[], string[], string[], string[], string[], string[]];
  digest: { heading: string; items: [[string, string], [string, string], [string, string]]; closing: string };
  threads: [string, string, string];
  chat: {
    ask: string; excerpt: string; answerLead: string;
    intro: [string, string, string]; points: [[string, string], [string, string, string, string], [string, string]];
    outro: [string, string, string, string, string];
    follow: string; followLead: string; followBody: string;
    other: [string, string];
    digestAsk: string; digestLead: string; digestBody: [string, string, string];
  };
  rules: { pattern: string; scope: string };
};

const ZH: Copy = {
  ui: { digest: "简报", feeds: /^订阅/, feedTree: "订阅", search: "搜索文章", searchDialog: "搜索文章", settings: "设置", suggestedRules: "建议规则" },
  folders: ["技术", "中文", "科技新闻"],
  folderFeeds: ["纸上谈码", "慢读札记", "开源前哨", "闲聊广场"],
  interest: "本地优先",
  localFirst: {
    title: "本地优先软件的第二个十年：同步引擎、CRDT 与单文件数据库",
    summary: "五个本地优先应用的架构复盘：数据先落本机 SQLite，同步只是后台的一层。离线可用、可迁移，服务关停也不丢数据。",
  },
  waylandHead: "主流发行版默认会话全面转向 Wayland",
  waylandRep: { title: "Wayland 成为默认会话之后：X11 会话进入维护模式", summary: "多个发行版的新版本不再默认安装 X11 会话；XWayland 继续承担旧应用的兼容。" },
  waylandOld: "X.Org 会话在新版发行版中改为可选组件",
  waylandNew: "国产发行版跟进 Wayland：输入法与远程桌面是最后两块短板",
  others: [
    { title: "一个人维护 RSS 阅读器十年：我为什么坚持不做算法推荐", summary: "规则比黑盒更可控：关键词、作者与来源三类过滤覆盖了九成需求，剩下的交给手动收藏。" },
    { title: "SQLite 为什么适合做应用的文件格式", summary: "单文件、带事务、可查询，比自定义二进制格式或一堆 JSON 更耐用，也更容易备份与迁移。" },
    { title: "长文阅读体验：行宽、字号与段落节奏", summary: "中文正文每行 30–40 字最舒适；行高与段距一起调，比单独加大字号更有效。" },
    { title: "大家现在都用什么方式管理信息流？", summary: "从 RSS、Newsletter 到稍后读，讨论了两百多楼。" },
    { title: "在浏览器里跑小模型：WebGPU 推理的现状与限制", summary: "量化后的小模型已经能在笔记本浏览器里流畅对话，瓶颈在显存与首次加载。" },
  ],
  timelineOnly: ["终端里的富文本：现代终端模拟器支持了哪些转义序列", "编译器性能：下一阶段的工作计划", "把 Newsletter 转成 RSS 的几种方法"],
  lede: "本地优先软件回潮，Linux 桌面全面转向 Wayland，浏览器端推理走向日常。",
  cluster: {
    overview: "**多个主流发行版**在新版本中把 Wayland 设为唯一默认会话，X11 会话改为可选安装；XWayland 继续兼容旧应用。",
    overviewCheck: "多个主流发行版",
    oldLine: "X.Org 会话改为可选组件",
    repLine: "X11 会话进入维护模式",
    addition: "国内发行版的进度取决于输入法框架与远程桌面协议的适配。",
    conflict: "X11 会话还会维护多久说法不一：一说两年，一说跟随上游。",
  },
  reader: {
    summary: "作者复盘了五个本地优先应用：数据先写进本机单文件数据库，同步引擎只在后台合并变更。结论是离线体验与数据可迁移性带来的收益，远大于同步实现的复杂度。",
    intro: "十年前，「本地优先」还是一篇论文里的设想：软件应该像纸笔一样，数据始终在你手里，网络只是锦上添花。今天，越来越多的应用把这句话当成了架构的起点。",
    h1: "数据先落在本机",
    p1a: "五个应用的共同点是：所有写入先进本机的 ",
    p1b: " 文件，界面只读本地状态；同步引擎在后台把变更合并到其他设备。断网时一切照常，只是暂时看不到别处的修改。",
    quote: "离线不是一种故障模式，而是默认状态。",
    h2: "同步只是一层",
    items: [
      ["CRDT", "：适合文本与列表，合并无冲突，但元数据会持续增长。"],
      ["操作日志", "：服务端排序，实现简单，需要在线才能确定最终顺序。"],
      ["整库快照", "：最朴素也最可靠，适合单人多设备。"],
    ],
    outro: "作者的结论是：先把单机体验做到极致，再决定要哪一种同步。多数个人工具甚至不需要实时协作。",
  },
  briefs: [
    ["新一轮发行版更新里，Wayland 会话成了唯一的默认选项。X11 会话仍可从软件源安装，但不再随桌面一起预装。", "XWayland 继续负责旧应用的兼容，大部分用户不会察觉到差别。"],
    ["十年前我写下第一行代码时，就决定所有文章都存进一个 SQLite 文件：备份就是复制，迁移就是拷走。", "过滤规则只有三类：关键词、作者与来源。它们覆盖了九成需求，而且每一次隐藏都能说清原因。"],
    ["应用的文件格式要同时满足可靠、可查询与可迁移。SQLite 用一个文件就做到了这三点。", "相比自定义二进制格式，它自带事务；相比一堆 JSON，它支持索引与全文检索，备份时用 VACUUM INTO 即可得到一致的快照。"],
    ["楼主用了十年 RSS，最近开始把 Newsletter 也转成订阅源，想听听大家的做法。"],
    ["中文正文每行 30 到 40 个字最舒适。比加大字号更有效的，是让行高与段距一起变化。"],
    ["量化后的小模型已经能在笔记本浏览器里流畅对话，权重缓存在本地，第二次打开几乎不用等待。", "瓶颈主要在显存与首次加载的下载量。"],
  ],
  digest: {
    heading: "今日三条主线",
    items: [
      ["本地优先回潮", "：数据先落本机单文件数据库，同步退到后台；SQLite 被越来越多地当作应用文件格式。"],
      ["Wayland 成为默认", "：主流发行版把 X11 会话改为可选，国内发行版卡在输入法与远程桌面。"],
      ["浏览器端推理", "：量化小模型已能在笔记本浏览器里流畅运行，瓶颈转向显存与首次加载。"],
    ],
    closing: "共同的方向是「把计算和数据留在用户自己的机器上」。",
  },
  threads: ["本地优先应用该选哪种同步方案", "对比三篇 Wayland 报道的分歧", "整理本周值得精读的长文"],
  chat: {
    ask: "我想给个人笔记工具加多设备同步，这篇里提到的三种方案该怎么选？",
    excerpt: "整库快照：最朴素也最可靠，适合单人多设备。",
    answerLead: "单人多设备，先用整库快照。",
    intro: ["如果只是你一个人在几台设备之间用，", "先选整库快照", "，理由有三："],
    points: [
      ["冲突少", "：同一时间通常只在一台设备上编辑，按修改时间取新即可。"],
      ["实现小", "：备份与同步是同一份文件，可以直接复用 ", "VACUUM INTO", " 生成快照。"],
      ["可迁移", "：快照就是普通 SQLite 文件，换工具也带得走。"],
    ],
    outro: ["等需要多人实时协作时再引入 CRDT。原文对元数据增长的提醒值得先看一遍，见 ", "同步只是一层", "；", "SQLite 作为文件格式", " 一文也给了快照的具体做法。"],
    follow: "库变大以后，每次传整个文件会不会太慢？",
    followLead: "几十 MB 以内不用担心。",
    followBody: "个人笔记库通常只有几十 MB，压缩后整库上传只要几秒。真到了几百 MB，再按表记录修改时间、只传变化的行，同步逻辑仍然留在本机。",
    other: ["三篇报道对 X11 维护期限的说法为什么不一样？", "从这周收藏里挑三篇适合周末精读的长文。"],
    digestAsk: "展开第二条：国内发行版具体卡在哪里？",
    digestLead: "主要是输入法与远程桌面。",
    digestBody: ["据 ", "开源前哨的报道", "，卡点集中在两处：输入法框架要改走 Wayland 的文本输入协议，候选框定位仍有问题；远程桌面要从截屏转发换成门户接口，企业里常用的方案还没跟上。"],
  },
  rules: { pattern: "本地优先", scope: "科技新闻" },
};

const EN: Copy = {
  ui: { digest: "Digest", feeds: /^Feeds/, feedTree: "Feeds", search: "Search", searchDialog: "Search articles", settings: "Settings", suggestedRules: "Suggested rules" },
  folders: ["Engineering", "Long reads", "Tech news"],
  folderFeeds: ["Margin Notes", "Slow Reader", "Open Source Outpost", "The Lounge"],
  interest: "local-first",
  localFirst: {
    title: "The second decade of local-first software: sync engines, CRDTs and single-file databases",
    summary: "A look back at five local-first apps: data lands in a local SQLite file first and sync is just a background layer. They work offline, move between tools and survive the service shutting down.",
  },
  waylandHead: "Major distributions switch their default session to Wayland",
  waylandRep: { title: "After Wayland becomes the default: the X11 session goes into maintenance mode", summary: "New releases of several distributions no longer install the X11 session by default; XWayland keeps older apps working." },
  waylandOld: "X.Org session becomes an optional package in new distribution releases",
  waylandNew: "Regional distributions follow Wayland: input methods and remote desktop are the last two gaps",
  others: [
    { title: "Ten years of maintaining an RSS reader alone: why I never added algorithmic recommendations", summary: "Rules are easier to control than a black box: filters on keywords, authors and sources cover nine in ten needs, and the rest is manual bookmarking." },
    { title: "Why SQLite makes a good application file format", summary: "One file, transactions and queries: it outlasts a custom binary format or a pile of JSON, and is easier to back up and move." },
    { title: "Reading long articles: line length, font size and paragraph rhythm", summary: "Lines of 60–75 characters read most comfortably; adjusting line height and paragraph spacing together helps more than a bigger font." },
    { title: "How does everyone manage their reading feeds these days?", summary: "From RSS and newsletters to read-later apps, the thread has passed two hundred replies." },
    { title: "Running small models in the browser: where WebGPU inference stands", summary: "Quantized small models now chat smoothly in a laptop browser; the limits are GPU memory and the first download." },
  ],
  timelineOnly: ["Rich text in the terminal: which escape sequences modern terminal emulators support", "Compiler performance: the plan for the next phase", "A few ways to turn newsletters into RSS"],
  lede: "Local-first software is back, the Linux desktop moves to Wayland, and in-browser inference becomes everyday.",
  cluster: {
    overview: "**Several major distributions** make Wayland the only default session in their new releases, and the X11 session becomes an optional install; XWayland keeps older apps working.",
    overviewCheck: "Several major distributions",
    oldLine: "X.Org session becomes optional",
    repLine: "X11 session enters maintenance mode",
    addition: "Progress in regional distributions depends on input-method frameworks and remote-desktop protocols catching up.",
    conflict: "Reports differ on how long the X11 session will be maintained: one says two years, another says as long as upstream does.",
  },
  reader: {
    summary: "The author looks back at five local-first apps: data is written to a local single-file database first, and the sync engine merges changes in the background. The gain in offline use and data portability far outweighs the complexity of sync.",
    intro: "Ten years ago, “local-first” was an idea in a paper: software should work like pen and paper, with your data always in your hands and the network as a bonus. Today more and more apps take that sentence as the starting point of their architecture.",
    h1: "Data lands on the device first",
    p1a: "What the five apps share: every write goes to a local ",
    p1b: " file first, and the interface reads only local state; the sync engine merges changes to other devices in the background. Offline, everything works as usual; you just don't see edits made elsewhere yet.",
    quote: "Offline is not a failure mode; it is the default.",
    h2: "Sync is just a layer",
    items: [
      ["CRDTs", ": good for text and lists and merge without conflicts, but their metadata keeps growing."],
      ["Operation logs", ": the server orders operations; simple to build, but you need to be online to know the final order."],
      ["Whole-database snapshots", ": the plainest and most reliable option, a good fit for one person on several devices."],
    ],
    outro: "The author's conclusion: make the single-device experience as good as it can be, then decide which kind of sync you need. Most personal tools don't even need real-time collaboration.",
  },
  briefs: [
    ["In the latest round of distribution updates, the Wayland session is the only default. The X11 session can still be installed from the repositories but no longer ships with the desktop.", "XWayland keeps older apps working, and most users won't notice the difference."],
    ["When I wrote the first line of code ten years ago, I decided every article would live in one SQLite file: backing up is copying, moving is taking the file with you.", "There are only three kinds of filter rules: keywords, authors and sources. They cover nine in ten needs, and every hidden item can say why."],
    ["An application file format has to be reliable, queryable and portable at once. SQLite does all three with a single file.", "Unlike a custom binary format it has transactions; unlike a pile of JSON it has indexes and full-text search, and VACUUM INTO gives you a consistent snapshot for backups."],
    ["I've used RSS for ten years and recently started turning newsletters into feeds too. How do you all do it?"],
    ["Lines of 60 to 75 characters read most comfortably. Changing line height and paragraph spacing together works better than a bigger font."],
    ["Quantized small models now chat smoothly in a laptop browser; the weights are cached locally, so the second launch is almost instant.", "The limits are mainly GPU memory and the size of the first download."],
  ],
  digest: {
    heading: "Three threads today",
    items: [
      ["Local-first is back", ": data lands in a local single-file database and sync moves to the background; more apps use SQLite as their file format."],
      ["Wayland becomes the default", ": major distributions make the X11 session optional, while regional ones are held up by input methods and remote desktop."],
      ["Inference in the browser", ": quantized small models run smoothly in a laptop browser, and the bottleneck moves to GPU memory and the first download."],
    ],
    closing: "The common direction: keep computation and data on the user's own machine.",
  },
  threads: ["Which sync approach suits a local-first app", "Where three Wayland reports disagree", "Long reads worth a weekend this week"],
  chat: {
    ask: "I want to add multi-device sync to my personal notes tool. How do I choose between the three approaches in this article?",
    excerpt: "Whole-database snapshots: the plainest and most reliable option, a good fit for one person on several devices.",
    answerLead: "For one person on several devices, start with whole-database snapshots.",
    intro: ["If it's just you moving between a few devices, ", "start with whole-database snapshots", ", for three reasons:"],
    points: [
      ["Few conflicts", ": you usually edit on one device at a time, so the newest copy wins."],
      ["Little code", ": backup and sync are the same file, and you can reuse ", "VACUUM INTO", " to make the snapshot."],
      ["Portable", ": a snapshot is an ordinary SQLite file that goes with you to any tool."],
    ],
    outro: ["Bring in CRDTs once you need real-time collaboration. The article's warning about growing metadata is worth reading first, in ", "Sync is just a layer", "; the ", "SQLite as a file format", " piece also shows how to take snapshots."],
    follow: "Once the database grows, won't sending the whole file every time be too slow?",
    followLead: "Not below a few dozen MB.",
    followBody: "A personal notes database is usually a few dozen MB, and uploading it compressed takes seconds. If it ever reaches hundreds of MB, record change times per table and send only the changed rows; the sync logic still stays on the device.",
    other: ["Why do the three reports give different dates for the end of X11 maintenance?", "Pick three long reads from this week's stars for the weekend."],
    digestAsk: "Expand the second item: what exactly holds up the regional distributions?",
    digestLead: "Mainly input methods and remote desktop.",
    digestBody: ["According to ", "Open Source Outpost", ", two things: input-method frameworks have to move to Wayland's text-input protocol and still place the candidate window wrongly, and remote desktop has to move from screen forwarding to portal interfaces, which common enterprise tools haven't done yet."],
  },
  rules: { pattern: "local-first", scope: "Tech news" },
};

const COPY: Record<DemoLanguage, Copy> = { "zh-CN": ZH, en: EN };

export type ReadmeDemo = { host: MockHostOptions; ui: Copy["ui"]; overviewCheck: string; firstThread: string };

export function readmeDemo(language: DemoLanguage): ReadmeDemo {
  const c = COPY[language];
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
  const day = (ms: number) => { const d = new Date(ms); return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };

  // ── 订阅 ──
  const feed = (id: number, title: string, folder_id: number, url: string, extra: Partial<FeedView> = {}): FeedView => ({
    id, title, url, folder_id, icon_href: "", error: null, noisy: false, retention_days: null, interval_minutes: null, last_fetched_at: null, next_fetch_at: null, limited_until: null,
    health: null, position: id, empty: false, ...extra,
  });
  const [f5, f6, f7, f8] = c.folderFeeds;
  const feeds: FeedTree = {
    folders: [
      { id: 1, name: c.folders[0], label: c.folders[0], indent: 0, position: 0, feeds: [
        feed(1, "Byte Harbor", 1, "https://byte-harbor.example/rss"),
        feed(2, "Kernel Ledger", 1, "https://kernel-ledger.example/rss"),
        feed(3, "Compiler Notes", 1, "https://compiler-notes.example/feed.xml"),
        feed(4, "Layout & Type", 1, "https://layout-type.example/feed"),
      ] },
      { id: 2, name: c.folders[1], label: c.folders[1], indent: 0, position: 1, feeds: [
        feed(5, f5, 2, "https://zhishang.example/feed"),
        feed(6, f6, 2, "https://mandu.example/atom.xml"),
        feed(7, f7, 2, "https://kaiyuan.example/rss"),
        feed(8, f8, 2, "https://xianliao.example/index.xml", { noisy: true, health: { code: "feed_health_noisy", params: { window_days: 14 } } }),
      ] },
      { id: 3, name: c.folders[2], label: c.folders[2], indent: 0, position: 2, feeds: [
        feed(9, "Circuit Review", 3, "https://circuit-review.example/feed"),
        feed(10, "Penguin Bench", 3, "https://penguin-bench.example/rss"),
      ] },
    ],
    uncategorized: [feed(11, "Local-first Lab", 0, "https://localfirst-lab.example/index.xml")],
    folder_combo: [...c.folders],
  };
  const feedTitle = (id: number) => [...feeds.folders.flatMap(f => f.feeds), ...feeds.uncategorized].find(f => f.id === id)?.title ?? "";

  // ── 今日焦点 ──
  const article = (id: number, title: string, feed_id: number, published_at: number, extra: Partial<ArticleView> = {}): ArticleView => ({
    id, title, feed_id, feed_title: feedTitle(feed_id), feed_icon_href: "", published_at, url: `https://example.org/${id}`,
    read: false, starred: false, score: 0, summary: "", row_kind: 0, cluster_id: 0, cluster_size: 1, cluster_label: "",
    cluster_span: "", interest_reason: null, rank_reasons: [], noisy: false, spot_check: false, explore: false, ...extra,
  });
  const interest = (pattern: string) => ({ code: "interest_match" as const, params: { pattern } });

  const localFirst = article(101, c.localFirst.title, 1, ago(18), { score: 14, interest_reason: interest(c.interest), summary: c.localFirst.summary });
  const waylandRep = article(901, c.waylandRep.title, 2, ago(42), { score: 8, cluster_id: 900, cluster_size: 3, summary: c.waylandRep.summary });
  const waylandOld = article(902, c.waylandOld, 10, ago(5 * 60), { cluster_id: 900, cluster_size: 3, read: true });
  const waylandNew = article(903, c.waylandNew, 7, ago(95), { cluster_id: 900, cluster_size: 3 });
  const waylandHead = article(900, c.waylandHead, 0, 0, {
    feed_title: "", published_at: null, row_kind: 1, cluster_id: 900, cluster_size: 3,
    cluster_label: c.waylandHead, cluster_span: `${day(yesterdayAt(12, 0))} → ${day(now)}`,
    cluster_sources: [{ feed_id: 2, title: "Kernel Ledger", icon_href: "" }, { feed_id: 10, title: "Penguin Bench", icon_href: "" }, { feed_id: 7, title: f7, icon_href: "" }],
  });
  const [o1, o2, o3, o4, o5] = c.others;
  const solo = article(102, o1.title, 5, ago(2 * 60 + 10), { score: 6, starred: true, summary: o1.summary });
  const sqlite = article(103, o2.title, 6, ago(3 * 60 + 25), { score: 5, interest_reason: interest("SQLite"), summary: o2.summary });
  const typeset = article(104, o3.title, 4, ago(5 * 60 + 40), { score: 3, read: true, summary: o3.summary });
  const lounge = article(105, o4.title, 8, yesterdayAt(21, 40), { score: -3, noisy: true, summary: o4.summary });
  const webgpu = article(106, o5.title, 9, yesterdayAt(9, 12), { score: 4, summary: o5.summary });
  const focusArticles = [localFirst, waylandHead, waylandRep, solo, sqlite, typeset, lounge, webgpu];

  const clusterRef = (row: ArticleView, one_liner = ""): ClusterArticleView => ({
    article_id: row.id, feed_id: row.feed_id, feed_title: row.feed_title, icon_href: "", title: row.title, published_at: row.published_at, one_liner,
  });
  const waylandSummary: ClusterSummaryView = {
    cluster_id: 900, state: "ready", model_ready: true, estimate_tokens: 2000, basis: 3, generated_at: ago(6), fresh: [],
    members: [clusterRef(waylandOld, c.cluster.oldLine), clusterRef(waylandRep, c.cluster.repLine), clusterRef(waylandNew)],
    overview: c.cluster.overview,
    additions: [{ text: c.cluster.addition, sources: [clusterRef(waylandNew)] }],
    conflicts: [{ text: c.cluster.conflict, sources: [clusterRef(waylandOld), clusterRef(waylandRep)] }],
  };

  // ── 阅读器 ──
  const t = (text: string) => ({ t: "t", text });
  const r = c.reader;
  const reader: ReaderView = {
    id: localFirst.id, source_hash: "readme-local-first", title: localFirst.title,
    feed_id: localFirst.feed_id, feed_title: localFirst.feed_title,
    meta: "Byte Harbor", feed_icon_href: "", url: "https://example.org/local-first",
    summary: r.summary, score: 14, feedback: 0, starred: false,
    reasons: [
      { rule_id: 11, dimension: "title_kw", snippet: c.interest, code: "rule_title_kw", params: { pattern: c.interest }, weight: 10 },
      { rule_id: 12, dimension: "body_phrase", snippet: "CRDT", code: "rule_body_phrase", params: { pattern: "CRDT" }, weight: 2 },
      { rule_id: 0, dimension: "interest", snippet: "", code: "interest_match", params: { pattern: c.interest }, weight: 2 },
    ],
    media: [], embed_covers: [], translated_title: "", translated_document: null, translation_ready: false, translation_note: "",
    truncated: false, pruned: false, summary_stale: false,
    document: {
      v: 1,
      blocks: [
        { t: "p", inlines: [t(r.intro)] },
        { t: "h", level: 2, inlines: [t(r.h1)] },
        { t: "p", inlines: [t(r.p1a), { t: "c", text: "SQLite" }, t(r.p1b)] },
        { t: "q", inlines: [t(r.quote)] },
        { t: "h", level: 2, inlines: [t(r.h2)] },
        ...r.items.map(([lead, rest]) => ({ t: "li", indent: 0, marker: "•", inlines: [{ t: "s", children: [t(lead)] }, t(rest)] })),
        { t: "pre", lang: "sql", text: "PRAGMA journal_mode = WAL;\nCREATE TABLE changes (id INTEGER PRIMARY KEY, site TEXT, clock INTEGER, op BLOB);" },
        { t: "p", inlines: [t(r.outro)] },
      ],
    },
  };
  // 其余文章的正文：搜索命中各自的段落。
  const brief = (row: ArticleView, paragraphs: string[]): ReaderView => ({
    ...reader, id: row.id, source_hash: `readme-${row.id}`, title: row.title, url: row.url, summary: row.summary, score: row.score,
    feed_id: row.feed_id, feed_title: row.feed_title, meta: row.feed_title, reasons: [], starred: row.starred,
    document: { v: 1, blocks: paragraphs.map(text => ({ t: "p", inlines: [t(text)] })) },
  });
  const readers: Record<number, ReaderView> = Object.fromEntries(
    [waylandRep, solo, sqlite, lounge, typeset, webgpu].map((row, index) => brief(row, c.briefs[index])).map(view => [view.id, view]),
  );

  // ── 简报 ──
  const source = (row: ArticleView, number: number): DigestSourceView => ({
    article_id: row.id, title: row.title, feed_title: row.feed_title, number, summary: row.summary || null, published_at: row.published_at, status: "available",
  });
  const sources = [source(localFirst, 1), source(sqlite, 2), source(waylandRep, 3), source(waylandNew, 4), source(webgpu, 5)];
  const digest: DigestView = {
    id: 101, period: "daily", period_start: new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime(),
    revision: "readme-digest", source_state: "verified", source_index: sources, unlocated_sources: [],
    body: "",
    items: [
      { block_id: 1, sources: [sources[0], sources[1]] },
      { block_id: 2, sources: [sources[2], sources[3]] },
      { block_id: 3, sources: [sources[4]] },
    ],
    document: {
      v: 1,
      blocks: [
        { t: "h", level: 3, inlines: [t(c.digest.heading)] },
        ...c.digest.items.map(([lead, rest], index) => ({ t: "li", marker: `${index + 1}.`, indent: 0, inlines: [{ t: "s", children: [t(lead)] }, t(rest)] })),
        { t: "p", inlines: [t(c.digest.closing)] },
      ],
    },
  };
  const previousDigest: DigestView = { ...digest, id: 100, revision: "readme-digest-prev", period_start: digest.period_start - 86_400_000 };

  // ── 对话 ──
  const message = (id: number, role: string, text: string, extra: Partial<ChatMessageView> = {}): ChatMessageView => ({
    id, role, text, streaming: false, cite_article_id: 0, cite_title: "", citations: [], document: null, ...extra,
  });
  const threads = [
    threadFixture({ id: 7, title: c.threads[0], updated_at: ago(3) }),
    threadFixture({ id: 8, title: c.threads[1], updated_at: ago(60) }),
    threadFixture({ id: 9, title: c.threads[2], updated_at: yesterdayAt(22, 5) }),
  ];
  const ch = c.chat;
  const [[p1, p1rest], [p2, p2a, p2code, p2b], [p3, p3rest]] = ch.points;
  const messages: Record<number, ChatMessageView[]> = {
    7: [
      message(1, "user", ch.ask, {
        citations: [
          { kind: "article", article_id: localFirst.id, title: localFirst.title, feed_title: "Byte Harbor", url: localFirst.url, excerpt: "", block_id: 0 },
          { kind: "excerpt", article_id: localFirst.id, title: localFirst.title, feed_title: "Byte Harbor", url: localFirst.url, excerpt: ch.excerpt, block_id: 7 },
        ],
      }),
      message(2, "assistant", ch.answerLead, {
        document: {
          v: 1,
          blocks: [
            { t: "p", inlines: [t(ch.intro[0]), { t: "s", children: [t(ch.intro[1])] }, t(ch.intro[2])] },
            { t: "li", indent: 0, marker: "1.", inlines: [{ t: "s", children: [t(p1)] }, t(p1rest)] },
            { t: "li", indent: 0, marker: "2.", inlines: [{ t: "s", children: [t(p2)] }, t(p2a), { t: "c", text: p2code }, t(p2b)] },
            { t: "li", indent: 0, marker: "3.", inlines: [{ t: "s", children: [t(p3)] }, t(p3rest)] },
            { t: "p", inlines: [t(ch.outro[0]), { t: "a", href: `rrss://article/${localFirst.id}`, children: [t(ch.outro[1])] }, t(ch.outro[2]), { t: "a", href: "rrss://article/103", children: [t(ch.outro[3])] }, t(ch.outro[4])] },
          ],
        },
      }),
      message(5, "user", ch.follow),
      message(6, "assistant", ch.followLead, { document: { v: 1, blocks: [{ t: "p", inlines: [t(ch.followBody)] }] } }),
    ],
    8: [message(3, "user", ch.other[0])],
    9: [message(4, "user", ch.other[1])],
    301: [
      message(11, "user", ch.digestAsk),
      message(12, "assistant", ch.digestLead, {
        document: { v: 1, blocks: [
          { t: "p", inlines: [t(ch.digestBody[0]), { t: "a", href: "rrss://article/903", children: [t(ch.digestBody[1])] }, t(ch.digestBody[2])] },
        ] },
      }),
    ],
  };

  // ── 订阅时间线 ──
  const timeline: ArticleView[] = [
    localFirst, waylandRep, waylandNew, solo, sqlite,
    article(107, c.timelineOnly[0], 1, ago(4 * 60)),
    waylandOld, typeset,
    article(108, c.timelineOnly[1], 3, yesterdayAt(23, 5), { read: true }),
    webgpu,
    article(109, c.timelineOnly[2], 5, yesterdayAt(8, 30), { read: true }),
  ];

  const rules: RulesPage = {
    ...MOCK_RULES,
    rules: [
      { ...MOCK_RULES.rules[0], id: 11, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", pattern: c.rules.pattern },
      { ...MOCK_RULES.rules[1], id: 13, summary: { code: "rule_summary", params: { effect: "hide" } }, scope_name: "", pattern: language === "en" ? "hiring" : MOCK_RULES.rules[1].pattern },
      { ...MOCK_RULES.rules[2], id: 14, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "Byte Harbor", scope_id: 1 },
      { ...MOCK_RULES.rules[0], id: 12, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", dimension_id: "body_phrase", pattern: "CRDT", weight: 2 },
      { ...MOCK_RULES.rules[3], id: 15, summary: { code: "rule_summary", params: { effect: "hide" } }, scope_name: c.rules.scope, scope_id: 3, enabled: true },
    ],
    folders: feeds.folders.map(f => ({ id: f.id, label: f.label })),
    feeds: feeds.folders.flatMap(f => f.feeds).map(f => ({ id: f.id, label: f.title })),
  };

  const host: MockHostOptions = {
    systemLanguage: language,
    focus: { ...MOCK_FOCUS, digest_banner: "daily", digest_lede: c.lede, articles: focusArticles, has_more: false },
    feeds, threads, messages, digests: [digest, previousDigest],
    reader, readers, clusterArticles: { 900: [waylandOld, waylandRep, waylandNew] }, clusterSummaries: { 900: waylandSummary },
    timelineArticles: timeline, rules,
  };
  return { host, ui: c.ui, overviewCheck: c.cluster.overviewCheck, firstThread: c.threads[0] };
}
