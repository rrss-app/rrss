import { threadFixture } from "../src/test/fixtures/chat";
import { applyFeedTreeOrder } from "../src/test/treeOrder";
import {
  aiPageFixture, bindingsFixture, generatedConfigFixture, modelPoolFixture, networkProxyFixture, piConfigFixture, mcpFixture, piPageFixture, piProbeFixture, poolEntryFixture, providerFixture,
  translatePageFixture,
} from "../src/test/fixtures/settings";
import type { Locator, Page } from "@playwright/test";
import { createSettingsSchema } from "../src/settings/registry";
import type {
  UiError,
  ClusterSummaryView,
  WebVerifyStateEvent,
  SiteRulesPage,
  SiteRuleTestView,
  LinkSafety,
  OpenWebLinkView,
  FulltextError,
  NetworkProxyView,
  SaveNetworkProxyArgs,
  AiPage,
  ThreadFolderView,
  BackupPage,
  CapabilitySupport,
  ModelPoolView,
  PoolEntryView,
  RetentionPage,
  ArticleView,
  ChatMessageView,
  CitationView,
  DiagnosticsPage,
  DigestSourceView,
  DigestView,
  FeedTree,
  FocusPage,
  MemoryPage,
  ModelBindingsView,
  PiPage,
  PiProbe,
  ProviderPreset,
  SettingsSchema,
  ReaderView,
  VectorSpaceStatusView,
  HiddenPage,
  RuleStatsPage,
  RulesPage,
  StorageItemView,
  StoragePage,
  ThreadView,
  TranslatePage,
  SelectionTranslation,
  TranslationCommandError,
} from "../src/bindings";

// 带数据的模拟宿主：只在 Chromium 里替换 `__TAURI_INTERNALS__.invoke`，不启动 Pi、不请求模型、不碰日常库。
// 数据刻意覆盖收藏 / 负分 / 无摘要 / 兴趣标签 / 噪声源 / 聚类、待确认规则、待确认记忆、多端点等分支。

const MINUTE_MS = 60_000;
// 发布时间相对加载时刻，截图里的相对说法稳定；「昨天」按本地日历取固定钟点。
const minutesAgo = (minutes: number) => Date.now() - minutes * MINUTE_MS;
const yesterdayAt = (hours: number, minutes: number) => {
  const at = new Date();
  at.setDate(at.getDate() - 1);
  at.setHours(hours, minutes, 0, 0);
  return at.getTime();
};

const article = (
  id: number,
  title: string,
  feed_id: number,
  feed_title: string,
  published_at: number | null,
  extra: Partial<ArticleView> = {},
): ArticleView => ({
  id,
  title,
  feed_id,
  feed_title,
  feed_icon_href: "",
  published_at,
  url: `https://example.test/${id}`,
  read: false,
  starred: false,
  score: 0,
  summary: "",
  row_kind: 0,
  cluster_id: 0,
  cluster_size: 1,
  cluster_label: "",
  cluster_span: "",
  interest_label: "",
  rank_reasons: [],
  noisy: false,
  spot_check: false,
  ...extra,
});

export const MOCK_FOCUS: FocusPage = {
  filter: "hide_faded",
  list_scope: "all",
  status: "",
  digest_banner: "今日简报 · 9月19日",
  digest_lede: "单库向量检索：sqlite-vec 与 FTS5 共存的取舍，十万篇以内不需要外部向量库。",
  has_more: false,
  ranking: {
    interest_boost: true,
    rerank: false,
    cluster_group: true,
    hide_noisy: false,
    interest_ready: true,
    ranking_note: "",
    hide_threshold: -10,
    theme: "light",
    rank_features: true,
    rank_source: true,
    rank_coverage: true,
    rank_freshness: true,
    rank_affinity: true,
    show_details: false,
  },
  articles: [
    article(1, "sqlite-vec 在桌面端单库里的取舍：WAL、FTS5 与向量检索如何共存", 2, "Compiler Notes", minutesAgo(12), {
      summary: "不必上 Qdrant：WAL + FTS5 已经够用到十万级，向量检索只在兴趣重排时参与。",
      score: 12,
      interest_label: " · 本地优先",
    }),
    article(2, "Tauri 2 的 IPC 命令默认在主线程执行", 3, "Byte Harbor", minutesAgo(60), {
      summary: "同步命令阻塞窗口，异步命令走线程池；桌面应用应把慢查询移出主线程。",
      score: 6,
      starred: true,
    }),
    article(3, "WebKitGTK 2.46 发布说明", 2, "Compiler Notes", minutesAgo(180), {
      read: true,
      score: 2,
    }),
    article(4, "我用旧平板做了一块桌面信息屏", 1, "闲聊广场", yesterdayAt(21, 40), {
      summary: "用一组小工具把刘海区变成状态栏，专门盯本地模型和 RSS 任务。",
      score: -4,
      noisy: true,
    }),
    article(5, "React 19 编译器在中大型应用里的实际收益", 4, "Gadget Wire Tech Weekly Digest Long Name", yesterdayAt(9, 12), {
      read: true,
      summary: "自动记忆化减少了约三成的重渲染，但对列表虚拟化几乎没有影响。",
    }),
    article(10, "", 0, "", null, {
      row_kind: 1,
      cluster_id: 10,
      cluster_size: 3,
      cluster_label: "Rust 1.97 发布与生态跟进",
      cluster_span: "09-17 → 09-19",
    }),
    article(6, "编译器 1.97.1 点发行说明", 2, "Compiler Notes", minutesAgo(80 * 60), {
      summary: "修复若干稳定版回归，推荐尽快升级。",
      score: 8,
      cluster_id: 10,
      cluster_size: 3,
    }),
  ],
};

export const MOCK_FEEDS: FeedTree = {
  folders: [
    {
      id: 1,
      name: "中文",
      label: "中文",
      indent: 0,
      position: 0,
      feeds: [{ id: 1, title: "闲聊广场", url: "https://xianliao.example/index.xml", error: "", noisy: true, folder_id: 1, retention_days: null, icon_href: "", position: 0,
        health: "noisy", health_label: "多数内容被隐藏", health_detail: "近 14 天大部分文章被规则隐藏或被踩。可以退订，或在规则页调整。", empty: false }],
    },
    {
      id: 2,
      name: "English",
      label: "English",
      indent: 0,
      position: 1,
      feeds: [
        { id: 2, title: "Compiler Notes", url: "https://compiler-notes.example/feed.xml", error: "", noisy: false, folder_id: 2, retention_days: null, icon_href: "", health: "", health_label: "", health_detail: "", position: 0, empty: false },
        { id: 3, title: "Byte Harbor", url: "https://byte-harbor.example/rss", error: "", noisy: false, folder_id: 2, retention_days: 0, icon_href: "", position: 1, empty: false,
          health: "stale", health_label: "47 天没有新文章", health_detail: "平时约 6 天一篇；最近一篇入库于 8 月 11 日 09:30。\n抓取正常，最近成功：今天 08:02" },
      ],
    },
  ],
  uncategorized: [
    { id: 4, title: "Gadget Wire Tech Weekly Digest Long Name", url: "http://192.0.2.10:1200/gadgetwire", error: "源暂时不可用（503）", noisy: false, folder_id: 0, retention_days: null, icon_href: "", position: 2,
      health: "failing", health_label: "连续失败 12 次 · 9 天", health_detail: "最近错误：源暂时不可用（503）\n最近成功：9 月 18 日 07:40", empty: false },
  ],
  folder_combo: ["中文", "English"],
};

export const MOCK_THREADS: ThreadView[] = [
  threadFixture({ id: 7, title: "查证 Rust 1.97 的回归修复" }),
  threadFixture({ id: 8, title: "Tauri IPC 线程模型" }),
];

const message = (id: number, role: string, text: string, extra: Partial<ChatMessageView> = {}): ChatMessageView => ({
  id,
  role,
  text,
  streaming: false,
  cite_article_id: 0,
  cite_title: "",
  citations: [],
  document: null,
  ...extra,
});

export const MOCK_MESSAGES: Record<number, ChatMessageView[]> = {
  7: [
    message(1, "user", "这次 1.97.1 修了哪些回归？和 1.97.0 相比要不要立刻升级？", {
      citations: [
        { kind: "article", article_id: 6, title: "编译器 1.97.1 点发行说明", feed_title: "Compiler Notes", url: "https://example.test/6", excerpt: "", block_id: 0 },
      ],
    }),
    message(2, "assistant", "主要修了三类问题。", {
      document: {
        v: 1,
        blocks: [
          { t: "p", inlines: [{ t: "t", text: "主要修了三类问题，建议在 CI 通过后升级：" }] },
          { t: "li", indent: 0, marker: "1.", inlines: [{ t: "s", children: [{ t: "t", text: "借用检查回归" }] }, { t: "t", text: "：部分闭包捕获在 1.97.0 被误判，见 " }, { t: "a", href: "rrss://article/6", children: [{ t: "t", text: "发行说明" }] }, { t: "t", text: "。" }] },
          { t: "li", indent: 0, marker: "2.", inlines: [{ t: "t", text: "Windows 上的 " }, { t: "c", text: "cargo doc" }, { t: "t", text: " 路径处理。" }] },
          { t: "li", indent: 0, marker: "3.", inlines: [{ t: "t", text: "Linker 参数顺序导致的链接失败。" }] },
          { t: "p", inlines: [{ t: "t", text: "如果你依赖 " }, { t: "c", text: "async-trait" }, { t: "t", text: "，先看第一条。" }] },
        ],
      },
    }),
  ],
  8: [message(3, "user", "Tauri 的同步命令为什么会卡住窗口？")],
  301: [
    message(11, "user", "展开第二条"),
    message(12, "assistant", "第二条讨论阅读器按需加载。", {
      document: {
        v: 1,
        blocks: [
          { t: "p", inlines: [{ t: "t", text: "第二条讨论阅读器按需加载：长文只在进入视口时请求图片，并保留原文与译文的段落对应。请查看 " }, { t: "a", href: "rrss://article/2", children: [{ t: "t", text: "原文" }] }, { t: "t", text: " 核对具体实现。" }] },
        ],
      },
    }),
  ],
};

const mockDigestSources: DigestSourceView[] = [
  { article_id: 1, title: "sqlite-vec 在桌面端单库里的取舍", feed_title: "Compiler Notes", number: 1, summary: "单库向量检索", published_at: null, status: "available" },
  { article_id: 2, title: "Tauri 2 的 IPC 命令默认在主线程执行", feed_title: "Byte Harbor", number: 2, summary: null, published_at: null, status: "available" },
  { article_id: 3, title: "WebKitGTK 2.46 发布说明", feed_title: "Compiler Notes", number: 3, summary: null, published_at: null, status: "available" },
  { article_id: 5, title: "React 19 编译器在中大型应用里的实际收益", feed_title: "Gadget Wire Tech Weekly Digest Long Name", number: 4, summary: null, published_at: null, status: "available" },
];

export const MOCK_DIGESTS: DigestView[] = [
  {
    id: 101,
    period: "daily",
    period_label: "日报",
    period_start: Date.parse("2025-09-19T00:00:00+08:00"),
    date_label: "9月19日",
    body: "### 今日三条主线\n\n- **单库向量检索**：sqlite-vec 与 FTS5 共存的取舍，十万篇以内不需要外部向量库。\n- **桌面 IPC 线程模型**：Tauri 同步命令阻塞主线程，WebKitGTK 新版本改善了滚动合成。\n- **React 编译器**：自动记忆化对虚拟列表帮助有限，重点仍是数据层。\n\n今天的共同主题是「把慢的事情移出热路径」。",
    revision: "mock-daily-revision-101",
    source_state: "verified",
    source_note: "",
    source_index: mockDigestSources,
    unlocated_sources: [],
    items: [
      { block_id: 1, sources: [mockDigestSources[0]] },
      { block_id: 2, sources: [mockDigestSources[1], mockDigestSources[2]] },
      { block_id: 3, sources: [mockDigestSources[3]] },
    ],
    document: {
      v: 1,
      blocks: [
        { t: "h", level: 3, inlines: [{ t: "t", text: "今日三条主线" }] },
        { t: "li", marker: "1.", indent: 0, inlines: [{ t: "s", children: [{ t: "t", text: "单库向量检索" }] }, { t: "t", text: "：sqlite-vec 与 FTS5 共存的取舍，十万篇以内不需要外部向量库。" }] },
        { t: "li", marker: "2.", indent: 0, inlines: [{ t: "s", children: [{ t: "t", text: "桌面 IPC 线程模型" }] }, { t: "t", text: "：Tauri 同步命令阻塞主线程，WebKitGTK 新版本改善了滚动合成。" }] },
        { t: "li", marker: "3.", indent: 0, inlines: [{ t: "s", children: [{ t: "t", text: "React 编译器" }] }, { t: "t", text: "：自动记忆化对虚拟列表帮助有限，重点仍是数据层。" }] },
        { t: "p", inlines: [{ t: "t", text: "今天的共同主题是「把慢的事情移出热路径」。" }] },
      ],
    },
  },
  {
    id: 100,
    period: "daily",
    period_label: "日报",
    period_start: Date.parse("2025-09-18T00:00:00+08:00"),
    date_label: "9月18日",
    body: "### 昨日主线\n\n昨天只有一条值得看的更新。",
    revision: "mock-daily-revision-100",
    source_state: "unavailable",
    source_note: "",
    source_index: [],
    unlocated_sources: [],
    items: [],
    document: { v: 1, blocks: [{ t: "h", level: 3, inlines: [{ t: "t", text: "昨日主线" }] }, { t: "p", inlines: [{ t: "t", text: "昨天只有一条值得看的更新。" }] }] },
  },
  {
    id: 200,
    period: "weekly",
    period_label: "周报",
    period_start: Date.parse("2025-09-15T00:00:00+08:00"),
    date_label: "9月15日 – 9月21日",
    body: "### 本周三条主线",
    revision: "mock-weekly-revision-200",
    source_state: "unavailable",
    source_note: "",
    source_index: [],
    unlocated_sources: [],
    items: [],
    document: { v: 1, blocks: [{ t: "h", level: 3, inlines: [{ t: "t", text: "本周三条主线" }] }] },
  },
];

const TESTED_AT = Date.UTC(2026, 8, 29, 1, 12);
export const MOCK_AI: AiPage = aiPageFixture({
  providers: [
    providerFixture({ name: "本机 Ollama", kind: "ollama", base_url: "http://127.0.0.1:11434/v1", last_test: { at: TESTED_AT, ok: true, note: "6" } }),
    providerFixture({ id: 2, name: "OpenRouter", kind: "openrouter", base_url: "https://openrouter.ai/api/v1", key: "set", key_hint: "fake",
      last_test: { at: TESTED_AT, ok: true, note: "14" } }),
  ],
  selected_id: 1,
  usage: {
    generation: { today: 32_400, month: 860_000, paused: false },
    embedding: { today: 412_000, month: 12_400_000, paused: false },
    chat: { today: 12_400, month: 410_000, paused: false },
  },
});

// 与后端预设表同形；只用于界面场景，默认值仍以 `provider_presets.rs` 为准。
export const MOCK_PRESETS: ProviderPreset[] = [
  { id: "openai", name: "OpenAI", base_url: "https://api.openai.com/v1", protocol: "chat", compat_json: "{}" },
  { id: "deepseek", name: "DeepSeek", base_url: "https://api.deepseek.com/v1", protocol: "chat", compat_json: "{}" },
  { id: "openrouter", name: "OpenRouter", base_url: "https://openrouter.ai/api/v1", protocol: "chat", compat_json: "{}" },
  { id: "ollama", name: "Ollama（本机）", base_url: "http://127.0.0.1:11434/v1", protocol: "chat", compat_json: "{}" },
  { id: "openai_compat", name: "自定义 OpenAI 兼容", base_url: "", protocol: "chat", compat_json: "{}" },
];

// 加进来的模型与各连接记住的目录（阶段 19-6 / 19-7）。
const FETCHED_AT = Date.UTC(2026, 8, 28, 1, 12);
const caps = (tools: CapabilitySupport, json: CapabilitySupport, image: CapabilitySupport = "unknown", reasoning: CapabilitySupport = "unknown") =>
  ({ tool_calling: tools, structured_output: json, image_input: image, reasoning });
export const MOCK_POOL = modelPoolFixture({
  pool: [
    poolEntryFixture({ provider_id: 1, model: "qwen2.5:14b", engine_context_window: 32000, chat: true, detected: caps("supported", "supported", "unsupported"), tested_at: TESTED_AT }),
    poolEntryFixture({ provider_id: 1, model: "llama3.2", detected: caps("unsupported", "unsupported"), tested_at: TESTED_AT }),
    poolEntryFixture({ provider_id: 1, model: "nomic-embed-text", detected: caps("unknown", "unknown") }),
    poolEntryFixture({ provider_id: 2, model: "anthropic/claude-sonnet-4", chat: true, display_name: "Sonnet", thinking: "low", engine_context_window: 200000,
      detected: caps("supported", "supported", "supported", "supported"), tested_at: TESTED_AT }),
    poolEntryFixture({ provider_id: 2, model: "google/gemini-2.5-flash" }),
  ],
  catalogs: [
    { provider_id: 1, models: ["deepseek-r1:14b", "gemma3:12b", "llama3.2", "nomic-embed-text", "qwen2.5:14b", "qwen2.5:7b"], fetched_at: FETCHED_AT },
    { provider_id: 2, models: ["anthropic/claude-haiku-4.5", "anthropic/claude-sonnet-4", "deepseek/deepseek-chat-v3.1", "google/gemini-2.5-flash",
      "google/gemini-2.5-pro", "meta-llama/llama-3.3-70b-instruct", "mistralai/mistral-small-3.2-24b-instruct", "moonshotai/kimi-k2-0905",
      "openai/gpt-4.1", "openai/gpt-4.1-mini", "openai/gpt-5", "openai/gpt-5-mini", "qwen/qwen3-235b-a22b-2507", "z-ai/glm-4.6"], fetched_at: FETCHED_AT },
  ],
});

// 各用途明确选的模型（阶段 19-7：没有主模型与「跟随」）；翻译还没选。
const TASK_TITLES: Array<[string, string]> = [
  ["focus_summary", "焦点摘要"], ["article_summary", "本篇摘要"], ["article_segment", "长文分段压缩"], ["digest_daily", "日 Digest"],
  ["digest_weekly", "周 Digest"], ["digest_pick_daily", "日 Digest 选题"], ["digest_pick_weekly", "周 Digest 选题"],
  ["cluster_summary", "相近报道综述"], ["memory_candidate", "记忆候选"], ["episode_compress", "情节压缩"], ["article_translate", "本篇翻译"], ["embed", "嵌入"], ["chat", "对话"],
];
const TASK_MODELS: Record<string, [number, string]> = {
  focus_summary: [1, "llama3.2"], article_summary: [1, "qwen2.5:14b"], article_segment: [1, "qwen2.5:14b"],
  cluster_summary: [1, "qwen2.5:14b"], digest_daily: [2, "anthropic/claude-sonnet-4"], digest_weekly: [2, "anthropic/claude-sonnet-4"], digest_pick_daily: [1, "qwen2.5:14b"],
  digest_pick_weekly: [1, "qwen2.5:14b"], memory_candidate: [1, "qwen2.5:14b"], episode_compress: [1, "qwen2.5:14b"],
  embed: [1, "nomic-embed-text"], chat: [2, "anthropic/claude-sonnet-4"],
};
export const MOCK_BINDINGS: ModelBindingsView = bindingsFixture({
  tasks: TASK_TITLES.map(([task, title]) => {
    const [provider_id, model] = TASK_MODELS[task] ?? [0, ""];
    const entry = MOCK_POOL.pool.find((e) => e.provider_id === provider_id && e.model === model);
    return { task, title, provider_id, model, capabilities: entry?.effective };
  }),
});
for (const e of MOCK_POOL.pool) e.uses = MOCK_BINDINGS.tasks.filter((t) => t.provider_id === e.provider_id && t.model === e.model).map((t) => t.task);

export const MOCK_RULES: RulesPage = {
  rules: [
    { id: 1, summary: "标题包含『Rust』时加分 · 全局", polarity: "like", dimension: "标题", dimension_id: "title_kw", pattern: "Rust", scope_label: "全局", scope_kind: "global", scope_id: 0, weight: 10, source: "user", enabled: true, pending: false, pack_id: "" },
    { id: 2, summary: "标题包含『招聘』时隐藏 · 全局", polarity: "dislike", dimension: "标题", dimension_id: "title_kw", pattern: "招聘", scope_label: "全局", scope_kind: "global", scope_id: 0, weight: 15, source: "preset", enabled: true, pending: false, pack_id: "dev-focus" },
    { id: 3, summary: "来自『Byte Harbor』、正文包含『local-first』时加分 · 仅此源", polarity: "like", dimension: "正文", dimension_id: "body_phrase", pattern: "local-first", scope_label: "源 · Byte Harbor", scope_kind: "feed", scope_id: 3, weight: 5, source: "suggest", enabled: false, pending: true, pack_id: "" },
    { id: 4, summary: "在『English』文件夹内、URL 匹配正则『/sponsored/』时隐藏 · 此文件夹及子文件夹", polarity: "dislike", dimension: "URL 正则", dimension_id: "url_re", pattern: "/sponsored/", scope_label: "文件夹 · English", scope_kind: "folder", scope_id: 2, weight: 20, source: "user", enabled: false, pending: false, pack_id: "" },
  ],
  presets: [
    { id: "dev-focus", name: "开发者关注", description: "偏向工程与工具链内容，压低招聘与营销。", rule_count: 12, enabled_count: 11, installed: true },
    { id: "chinese-media", name: "中文媒体降噪", description: "过滤标题党和转载。", rule_count: 8, enabled_count: 0, installed: false },
  ],
  folders: [{ id: 1, label: "中文" }, { id: 2, label: "English" }],
  feeds: [{ id: 1, label: "闲聊广场" }, { id: 2, label: "Compiler Notes" }, { id: 3, label: "Byte Harbor" }],
  hide_threshold: -10,
};

export const MOCK_MEMORY: MemoryPage = {
  memories: [
    { id: 1, kind: "偏好", content: "更关注本地优先与离线可用的工具，不喜欢纯营销稿。", pinned: true, pending: false, scope: "全局", strength: "strong" },
    { id: 2, kind: "事实", content: "日常在 WSL2 + WebKitGTK 上开发桌面应用。", pinned: false, pending: false, scope: "", strength: "normal" },
    { id: 3, kind: "偏好", content: "对 React 编译器的实际收益持保留态度，想看基准数据。", pinned: false, pending: true, scope: "对话 #7", strength: "weak" },
  ],
  interests: [
    { id: 1, label: "本地优先", weight: 1, enabled: true, has_embedding: true, hit_count: 14 },
    { id: 2, label: "SQLite", weight: 1, enabled: true, has_embedding: true, hit_count: 6 },
    { id: 3, label: "字体渲染", weight: 1, enabled: false, has_embedding: false, hit_count: 0 },
  ],
  inject_preview: "",
};

export const MOCK_PI: PiPage = piPageFixture({
  fetch_url: true,
  binary_source: "托管目录 ~/.local/share/rrss/bin/pi",
  manage_status: "托管 Pi 0.87.1",
  system_prompt: "<!-- rrss-system v3 -->\n你是 RRSS 阅读助手，只用本地工具和引用里实际存在的 ID。",
  thinking_levels: ["off", "low", "medium"],
  tools: [
    { name: "search_articles", label: "搜索文章", description: "本机全文检索，按标题与正文匹配。", default_description: "本机全文检索，按标题与正文匹配。", enabled: true, confirm: false },
    { name: "get_article", label: "读取原文", description: "只读正文前 3 段。", default_description: "按 ID 读取清洗后的正文块。", enabled: true, confirm: false },
    { name: "fetch_url", label: "外网查证", description: "抓取外部网页，每次调用前需要确认。", default_description: "抓取外部网页，每次调用前需要确认。", enabled: false, confirm: true },
  ],
  skills: [
    { name: "digest-followup", description: "简报追问：按条目编号定位来源。", bundled: true, enabled: true, loaded: true },
    { name: "my-notes", description: "用户自建：整理阅读笔记。", bundled: false, enabled: false, loaded: false },
  ],
  mcp_servers: [mcpFixture()],
});

export const MOCK_PI_PROBE: PiProbe = piProbeFixture({ binary_source: MOCK_PI.binary_source });

export const MOCK_SETTINGS: SettingsSchema = createSettingsSchema({ ai_daily_budget_tokens: "1000" });

export const MOCK_DIAG: DiagnosticsPage = {
  vec: "sqlite-vec v0.1.6",
  stats: "向量 3812/3900 已嵌入，待嵌入 chunk 88 / 文章 12。兴趣 2/3，记忆已嵌入 2。维度 [768d×3812] 模型 [nomic-embed-text:3812]",
  db: "库 /home/user/.local/share/rrss/rrss.db · 96,468,992 字节 · user_version 13 · wal · FTS 4,120 行",
  ai: "今日 token 18,204 / 200,000 · 摘要 41 · 简报 1",
  log: {
    directory: "/home/user/.local/share/rrss/logs", file: "/home/user/.local/share/rrss/logs/rrss.log",
    unavailable: null, current_size: "2.4 MB", total_size: "7.8 MB", total_bytes: 8178892,
    detailed: false, previous_unclean: null, can_clear: true,
  },
  scheduler: null,
};

const storageItem = (key: string, label: string, hint: string, bytes: number, size: string, extra: Partial<StorageItemView> = {}): StorageItemView => ({
  key, label, hint, bytes, size, free: false, action: "", action_label: "", passive: "", confirm_title: "", confirm: [], ...extra,
});
const MB = 1024 * 1024;
// 约一年用量的示例，与存储页样板的数字一致。
export const MOCK_STORAGE: StoragePage = {
  data_dir: "/home/user/.local/share/rrss",
  total: "4.44 GB",
  summary: "数据库 1.41 GB · 媒体缓存 632 MB · 对话引擎 114 MB · 自动备份 2.30 GB",
  groups: [
    {
      key: "database", label: "数据库", size: "1.41 GB", note: "含未写回的日志 12.0 MB", items: [
        storageItem("articles", "文章", "38,210 篇 · 标题、正文与阅读状态", 612 * MB, "612 MB"),
        storageItem("search_index", "全文索引", "供搜索使用，随文章增减", 441 * MB, "441 MB", { passive: "随文章变化" }),
        storageItem("vectors", "向量", "52,904 段 · 其中 8,120 段来自其他模型", 198 * MB, "198 MB", {
          action: "old_vectors", action_label: "清除旧模型向量", confirm_title: "清除旧模型向量",
          confirm: ["删除 8,120 段不是由当前模型 nomic-embed-text:v1.5 生成的向量。", "当前模型的向量不受影响；这些文章之后会按当前模型重新生成向量，远程模型会产生费用。"],
        }),
        storageItem("ai_cache", "AI 缓存", "9,870 条 · 30 天前的会自动清除", 31 * MB, "31.0 MB", {
          action: "ai_cache", action_label: "清空", confirm_title: "清空 AI 缓存",
          confirm: ["删除 9,870 条缓存的摘要和简报结果。", "之后遇到相同内容会重新调用模型，可能产生费用。"],
        }),
        storageItem("translations", "译文缓存", "412 篇", 23 * MB, "23.0 MB", {
          action: "translations", action_label: "清空", confirm_title: "清空译文缓存",
          confirm: ["删除 412 篇文章的译文。", "再次查看译文时会重新翻译，可能产生费用。"],
        }),
        storageItem("recommendation_logs", "推荐日志", "18,000 条 · 曝光、行为与阅读会话合计 · 保留 180 天", 3 * MB, "3.0 MB", {
          action: "recommendation_logs", action_label: "清空", confirm_title: "清空推荐日志",
          confirm: ["删除 18,000 条曝光、行为与阅读会话日志。", "文章、赞踩、收藏与阅读状态不受影响。"],
        }),
        storageItem("other", "其他", "对话与记忆 4.2 MB（86 个对话）· 设置、订阅与规则 3.3 MB · 简报 1.1 MB（143 期）", 8.6 * MB, "8.6 MB", { passive: "在各自页面管理" }),
        storageItem("legacy", "旧版遗留数据", "早期版本留下、已不再使用的表：translations、feed_sub_records", 0.4 * MB, "410 KB", {
          action: "legacy_tables", action_label: "删除", confirm_title: "删除旧版遗留数据",
          confirm: ["删除 translations、feed_sub_records 表。", "当前版本不再读取它们；译文已迁移到新表。"],
        }),
        storageItem("free", "空闲空间", "删除数据后留下的空位，压缩后还给磁盘。需要约 1.40 GB 临时空间，剩余 41.2 GB", 118 * MB, "118 MB", {
          free: true, action: "vacuum", action_label: "压缩数据库", confirm_title: "压缩数据库",
          confirm: ["把 118 MB 空闲空间还给磁盘。", "需要约 1.40 GB 临时磁盘空间（剩余 41.2 GB），大库可能需要一两分钟。", "期间刷新和后台作业会等待，可以继续阅读。"],
        }),
      ],
    },
    {
      key: "media", label: "媒体缓存", size: "632 MB", note: "", items: [
        storageItem("videos", "音视频缓存", "上限 512 MB", 388 * MB, "388 MB", {
          action: "videos", action_label: "清空", confirm_title: "清空音视频缓存", confirm: ["删除本地保存的音视频，释放约 388 MB。", "再播放时会重新下载。"],
        }),
        storageItem("images", "配图缓存", "上限 256 MB，打开文章时按需下载", 241 * MB, "241 MB", {
          action: "images", action_label: "清空", confirm_title: "清空配图缓存", confirm: ["删除本地保存的配图，释放约 241 MB。", "再打开文章时会重新下载。"],
        }),
        storageItem("untracked_media", "未登记文件", "14 个 · 中断的下载等，可以安全删除", 3.4 * MB, "3.4 MB", {
          action: "untracked_media", action_label: "清理", confirm_title: "清理未登记文件", confirm: ["删除媒体目录里没有登记的 14 个文件，约 3.4 MB。", "不影响任何文章。"],
        }),
      ],
    },
    {
      key: "engine", label: "对话引擎", size: "114 MB", note: "", items: [
        storageItem("pi_program", "对话引擎程序", "RRSS 托管的 Pi", 96 * MB, "96.0 MB"),
        storageItem("pi_sessions", "对话记录（引擎侧）", "续接对话所需，随备份保存", 18 * MB, "18.0 MB"),
      ],
    },
    {
      key: "backups", label: "自动备份", size: "2.30 GB", note: "", items: [
        storageItem("auto_backups", "自动备份", "2 份 · 只保留最近 5 份", 2355 * MB, "2.30 GB", { passive: "在「备份与恢复」管理" }),
      ],
    },
    {
      key: "logs", label: "运行日志", size: "7.8 MB", note: "保存在本机日志目录，不进备份。", items: [
        storageItem("logs", "运行日志", "用于排查问题，清空后继续记录", 7.8 * MB, "7.8 MB", {
          action: "logs", action_label: "清空", confirm_title: "清空运行日志？",
          confirm: ["删除现有的日志文件，之后继续记录新的日志。", "不影响订阅、文章和设置。"],
        }),
      ],
    },
  ],
  scanned_at: Date.now(),
};

export const MOCK_RETENTION: RetentionPage = {
  days: 0, default_days: 90, custom_feeds: 1, preview: "", preview_hint: "", can_run: false,
  confirm_title: "立即清理旧文章正文", confirm: [], confirm_ok: "备份并清理",
};

export const MOCK_BACKUP: BackupPage = {
  last_manual: "上次备份：9 月 20 日 22:40。",
  schedule: "off",
  auto_summary: "升级前自动 · 定时关闭",
  entries: [
    { path: "/home/user/.local/share/rrss/backups/rrss-upgrade-20260927-091200-000.zip", when: "今天 09:12", note: "升级前自动备份 · v14 · 1.21 GB", restorable: true },
    { path: "/home/user/.local/share/rrss/backups/rrss-scheduled-20260913-080500-000.zip", when: "9 月 13 日 08:05", note: "定时备份 · 1.09 GB", restorable: true },
  ],
  restore_notice: null,
  rules: 64,
  interests: 12,
  memories: 37,
};

export const MOCK_TRANSLATE: TranslatePage = translatePageFixture({
  backend: "complete",
});

export const MOCK_READER: ReaderView = {
  source_hash: "mock-reader-source",
  id: 1,
  title: "sqlite-vec 在桌面端单库里的取舍：WAL、FTS5 与向量检索如何共存",
  meta: "Compiler Notes · 12 分钟前 · 约 6 分钟", feed_icon_href: "",
  url: "https://example.test/1",
  summary: "作者对比了外部向量库与 sqlite-vec 的运维成本，结论是十万篇以内单库足够，关键是把嵌入和聚类移出刷新热路径。",
  score: 12,
  feedback: 0,
  reasons: [
    { rule_id: 1, dimension: "标题", snippet: "sqlite", label: "标题含 sqlite", weight: 10 },
    { rule_id: 0, dimension: "兴趣", snippet: "", label: "兴趣 · 本地优先", weight: 2 },
  ],
  media: [],
  translated_title: "",
  translated_document: null,
  translation_ready: false,
  translation_note: "",
  truncated: true, pruned: false, summary_stale: false,
  starred: false,
  embed_covers: [],
  document: {
    v: 1,
    blocks: [
      { t: "p", inlines: [{ t: "t", text: "桌面端的信息中枢通常只有一个用户、一台机器。这时引入独立的向量数据库，运维成本远高于收益。" }] },
      { t: "h", level: 2, inlines: [{ t: "t", text: "单库的三个组件" }] },
      { t: "p", inlines: [{ t: "t", text: "WAL 模式让读写并发；" }, { t: "c", text: "FTS5" }, { t: "t", text: " 负责关键词检索；" }, { t: "s", children: [{ t: "t", text: "sqlite-vec" }] }, { t: "t", text: " 提供近似向量检索。三者共用一个事务边界。" }] },
      { t: "q", inlines: [{ t: "t", text: "把慢的事情移出热路径，比把快的事情做得更快更重要。" }] },
      { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "嵌入在后台作业里按批执行" }] },
      { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "聚类只比较同批候选" }] },
      { t: "pre", lang: "sql", text: "PRAGMA journal_mode = WAL;\nCREATE VIRTUAL TABLE chunks USING vec0(embedding float[768]);" },
      { t: "p", inlines: [{ t: "t", text: "更多细节见 " }, { t: "a", href: "https://example.test/sqlite-vec", children: [{ t: "t", text: "项目主页" }] }, { t: "t", text: "。" }] },
    ],
  },
};

export type MockHostOptions = {
  /** 会话文件夹；默认没有。 */
  threadFolders?: ThreadFolderView[];
  /** 覆盖模型页快照的部分字段（用量、自动摘要状态与升级提示）。 */
  ai?: Partial<AiPage>;
  siteRules?: SiteRulesPage;
  siteRuleReport?: SiteRuleTestView;
  webExtractions?: Record<string, WebVerifyStateEvent>;
  webWindowError?: UiError | string;
  clearWebDataError?: UiError | string;
  webLinks?: Record<string, OpenWebLinkView | FulltextError>;
  linkSafety?: Record<string, LinkSafety>;
  digests?: DigestView[];
  /** 向量库状态；默认已绑定 nomic-embed-text。`allow_model_change` 总是取设置项的当前值。 */
  vectorSpace?: Partial<VectorSpaceStatusView>;
  /** 隐藏列表与规则统计；默认为空。 */
  hidden?: HiddenPage;
  ruleStats?: RuleStatsPage;
  /** 网络代理页的初始视图（含检测到的系统代理）。 */
  networkProxy?: NetworkProxyView;
  /** 加进来的模型；默认 `MOCK_POOL`。 */
  pool?: ModelPoolView;
  /** 引用托盘发送：只使用模拟消息与事件；可控制接受前失败 / 挂起。 */
  sendFailures?: number;
  holdSend?: boolean;
  focus?: FocusPage;
  feeds?: FeedTree;
  feedIconHrefs?: Record<number, string>;
  threads?: ThreadView[];
  messages?: Record<number, ChatMessageView[]>;
  reader?: ReaderView;
  /** 文末相关阅读（按文章 id）；未列出的文章没有相关阅读，同没有向量时一样。 */
  related?: Record<number, import("../src/bindings").RelatedArticleView[]>;
  /** 按文章 id 覆盖 `reader`。未列出的 id 仍用默认阅读视图，只改 id。 */
  readers?: Record<number, ReaderView>;
  /** 每次显式打开推进一版，读取快照不推进；用来验证正文版本变化。 */
  readerSequences?: Record<number, ReaderView[]>;
  /** 挂起显式打开；运行中也可设置 __mockHost.holdOpen，再 releaseOpen() 放行。 */
  holdOpen?: boolean;
  /** 挂起焦点查询；运行中可切换开关，再 releaseFocusPage() 放行。 */
  holdFocusPage?: boolean;
  /** 挂起简报期列表和正文，再 releaseDigest() 放行。 */
  holdDigest?: boolean;
  /** 可选的真实分页大小，续页使用 IPC 的排除列表。 */
  focusPageSize?: number;
  /** 合辑展开时由同一 focus_stream 查询插入的组内文章。 */
  clusterArticles?: Record<number, ArticleView[]>;
  /** 相近报道综述的读回结果（按簇）；没有时按组内文章给「未生成」。 */
  clusterSummaries?: Record<number, ClusterSummaryView>;
  /** `generate_cluster_summary` 的结果（按簇）；生成后也成为读回结果。没有时该命令报错。 */
  clusterGenerated?: Record<number, ClusterSummaryView>;
  /** 为 true 时生成综述挂起，直到页面调用 `__mockHost.releaseClusterSummary()`。 */
  holdClusterSummary?: boolean;
  /** 「相近报道综述」是否有可用模型，默认有。 */
  clusterSummaryModelReady?: boolean;
  /** 为 true 时 `summarize_article` 挂起，直到页面调用 `__mockHost.releaseSummary()`。 */
  holdSummary?: boolean;
  /** 摘要完成时返回的文本。未设置且未挂起时，该命令仍视为未模拟。 */
  summary?: string;
  translate?: TranslatePage;
  selectionTranslation?: string | TranslationCommandError;
  holdSelectionTranslation?: boolean;
  refreshFails?: boolean;
  piDelayMs?: number;
  backup?: BackupPage;
  /** 注入只影响一个局部渲染树的坏文档，供错误边界 e2e 验收。 */
  brokenBlocks?: {
    reader?: number[];
    digest?: number[];
    messages?: number[];
  };
  trainingFeatures?: import("../src/bindings").FeatureView[];
  rules?: RulesPage;
  /** 订阅时间线文章。不传时用焦点文章里 row_kind 为 0 的行。 */
  timelineArticles?: ArticleView[];
  /** `feeds_page` 每页条数，默认 50。 */
  feedsPageSize?: number;
  /** `feeds_page` 返回的 hidden_count，默认 0。 */
  feedsHiddenCount?: number;
  /** 覆盖注册表设置的初始值（如重启后保留的 `feeds_scope`）。 */
  settingValues?: Partial<Record<import("../src/settings/settingsSchemaData").SettingId, string>>;
  /** 自绘标题栏的窗口三键样式，默认 Windows。 */
  windowControls?: import("../src/bindings").WindowControlsStyle;
};

/** 在页面脚本之前注入模拟 IPC。返回的 `invoke` 同步产生结果，不涉及真实宿主。 */
export async function installMockHost(page: Page, options: MockHostOptions = {}) {
  await page.addInitScript(
    ({ webExtractions, webWindowError, clearWebDataError, siteRules, siteRuleReport, webLinks, linkSafety, related, focus, feeds, threads, threadFolders, messages, digests, sendFailures, holdSend, ai, bindings, pool, presets, rules, memory, pi, probe, piConfig, networkProxy, generated, settings, diag, storage, retention, backup, translate, reader, readers, readerSequences, holdOpen, holdFocusPage, holdDigest, focusPageSize, clusterArticles, clusterSummaries, clusterGenerated, holdClusterSummary, clusterSummaryModelReady, holdSummary, summary, selectionTranslation, holdSelectionTranslation, refreshFails, piDelayMs, feedIconHrefs, brokenBlocks, trainingFeatures, vectorSpace, hidden, ruleStats, timelineArticles, feedsPageSize, feedsHiddenCount, feedTreeOrder, windowControls }) => {
      const applyFeedTreeOrder = eval(`(${feedTreeOrder})`) as (tree: FeedTree, op: "move" | "restore", args: { items?: string[]; parentFolderId?: number | null; before?: string | null; layout?: { item: string; parent_folder_id: number | null; position: number }[] }) => { tree: FeedTree; layout: { item: string; parent_folder_id: number | null; position: number }[] };
      const collectWebArticle = (id: number) => {
        for (const value of Object.values(webLinks)) {
          if ("article_id" in value && value.article_id === id) value.temporary = false;
        }
      };
      const listeners = new Map<string, Set<number>>();
      // 与宿主一致：16 位以上显示首尾各 4 位，9–15 位只显示末 4 位。
      const keyHint = (key: string) => key.length >= 16 ? `${key.slice(0, 4)}…${key.slice(-4)}` : key.length > 8 ? `…${key.slice(-4)}` : "";
      const callbacks = new Map<number, (event: unknown) => void>();
      let callbackId = 0;
      let refreshOperation: import("../src/bindings").RefreshOperation | null = null;
      const state = { focus, feeds, threads, threadFolders, rules, memory, ai, bindings, pool, pi, probe, piConfig, networkProxy, settings, storage, retention, backup, translate, theme: "light",
        install: { state: "idle", received: 0, total: null as number | null, message: "", dir: "/home/user/.local/share/rrss/pi-pkg" },
        vectors: {
          effective: { id: 1, model: "nomic-embed-text", provider_name: "本机 Ollama", dims: 768, source_dims: 768, state: "active", chunks: 52904, stale_reason: null, checked_at: null },
          building: null, dims_setting: "auto", estimate_chunks: 52904, estimate_chars: 21000000, estimate_tokens: 10580800, local: true, ...vectorSpace,
        } as Record<string, any> };
      let remainingSendFailures = sendFailures;
      const pendingSends: Array<() => void> = [];
      const digestThreads = new Map<number, number>([[101, 301]]);
      let nextThreadId = Math.max(400, ...threads.map(t => t.id)) + 1;
      let nextMessageId = Math.max(0, ...Object.values(messages).flat().map(m => m.id)) + 1;
      const pendingSummaries: Array<(text: string) => void> = [];
      const pendingClusterSummaries: Array<() => void> = [];
      const pendingOpens: Array<() => void> = [];
      const pendingFocusPages: Array<() => void> = [];
      const pendingDigests: Array<() => void> = [];
      const pendingSelectionTranslations: Array<() => void> = [];
      // IR 校验会清洗未知节点；e2e 仅在测试浏览器内模拟该坏块的渲染器异常。
      const brokenDocuments: Array<{ raw: Record<string, unknown>; blocks: unknown[] }> = [];
      const brokenDocument = (document: unknown) => {
        const original = document as { v: number; blocks: unknown[] };
        const raw = { ...original, __rrssMockBroken: true, blocks: original.blocks.map((block, index) =>
          index === 0 && block && typeof block === "object" && "inlines" in block
            ? { ...block, inlines: [...(block as { inlines: unknown[] }).inlines, { t: "mock_broken_inline" }] }
            : block,
        ) };
        brokenDocuments.push({ raw, blocks: original.blocks });
        return raw;
      };
      const messageFor = (message: ChatMessageView) => brokenBlocks?.messages?.includes(message.id)
        ? { ...message, document: brokenDocument(message.document) }
        : message;
      // 与宿主一致：阅读视图和搜索结果带同一份本地图标，关掉「显示网站图标」后都为空。
      const iconFor = (feedId: number | undefined) =>
        feedId !== undefined && state.settings.values.find(v => v.id === "feed_icons_enabled")?.value !== "0" ? feedIconHrefs[feedId] ?? "" : "";
      const readerFor = (id: number) => {
        const base = readers[id] ?? { ...reader, id };
        const view = { ...base, feed_icon_href: iconFor(state.focus.articles.find(row => row.id === id)?.feed_id) || base.feed_icon_href };
        return brokenBlocks?.reader?.includes(id) ? { ...view, document: brokenDocument(view.document) } : view;
      };
      const normalizeSearch = (q: string) => {
        const trimmed = q.trim();
        return trimmed.startsWith('"') && trimmed.endsWith('"') && trimmed.length >= 2 ? trimmed.slice(1, -1).trim() : trimmed;
      };
      const foldAscii = (text: string) => text.replace(/[A-Z]/g, c => c.toLowerCase());
      const matchesSearch = (text: string, q: string) => !!q && foldAscii(text).includes(foldAscii(q));
      const inlinesText = (inlines: import("../src/lib/blockIr").Inline[]): string => inlines.map(node =>
        "text" in node ? node.text : "children" in node ? inlinesText(node.children) : "\n",
      ).join("");
      const searchBlocks = (id: number, query: string): import("../src/bindings").SearchHitBlocksView => {
        const view = readerFor(id);
        const q = normalizeSearch(query);
        const blocks = (view.document as import("../src/lib/blockIr").Document).blocks ?? [];
        const hits: import("../src/bindings").SearchBlockHitView[] = [];
        let total = 0;
        let section = "";
        if (!view.pruned) blocks.forEach((block, block_id) => {
          const text = "inlines" in block ? inlinesText(block.inlines) : block.t === "pre" ? block.text : "";
          if (text.trim()) {
            ++total;
            if (hits.length < 50 && matchesSearch(text, q)) hits.push({ block_id, ordinal: total, section, text: hitExcerpt(text, q, 400) });
          }
          if (block.t === "h") section = text;
        });
        return { source_hash: view.source_hash, total, title_match: matchesSearch(view.title, q), hits };
      };
      const graphemes = (text: string) => Array.from(new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(text));
      const truncateText = (text: string, max: number) => {
        const parts = graphemes(text);
        return parts.length > max ? text.slice(0, parts[max].index) + "…" : text;
      };
      const searchBody = (id: number) => {
        const view = readerFor(id);
        if (view.pruned) return "";
        return ((view.document as import("../src/lib/blockIr").Document).blocks ?? []).map(block =>
          "inlines" in block ? inlinesText(block.inlines) : block.t === "pre" ? block.text : "",
        ).filter(Boolean).join("\n");
      };
      // 同 core `match_window` / `hit_snippet` / `hit_excerpt`。
      const matchWindow = (text: string, q: string, context: number) => {
        const offset = q ? foldAscii(text).indexOf(foldAscii(q)) : -1;
        if (offset < 0) return null;
        const parts = graphemes(text);
        const first = parts.findIndex(p => p.index + p.segment.length > offset);
        const after = parts.findIndex(p => p.index >= offset + q.length);
        const start = Math.max(0, first - context);
        const end = Math.min(parts.length, (after < 0 ? parts.length : after) + context);
        return (start ? "…" : "") + text.slice(parts[start].index, end < parts.length ? parts[end].index : text.length) + (end < parts.length ? "…" : "");
      };
      const searchSnippet = (text: string, q: string) => matchWindow(text, q, 60) ?? truncateText(text, 120);
      const hitExcerpt = (text: string, q: string, max: number) =>
        graphemes(text).length <= max ? text : matchWindow(text, q, max / 2) ?? truncateText(text, max);
      // `args` 记下每个命令最近一次的参数，供用例核对；`emit` 让用例推送宿主事件（如 chat:confirm）。
      const harness = {
        webWindowError, clearWebDataError,
        webWindows: {} as Record<string, string>, nextWebWindow: 0,
        webViewBounds: [] as unknown[],
        webViewVisible: [] as { label: string; visible: boolean }[],
        calls: [] as string[], args: {} as Record<string, unknown>,
        /** 窗口三键的效果：最小化次数、最大化状态、是否已关闭。 */
        window: { minimized: 0, maximized: false, closed: false, hidden: false, trayLabels: null as unknown, closeChoices: [] as Array<{ choice: string; remember: boolean }> },
        /** 外部工具检测的结果（按服务名覆盖，没写的用 `default`）；浏览器登录挂起到 `finishMcpLogin`。 */
        mcpProbe: {
          default: { ok: true, elapsed_ms: 420, error: "", hint: "", output: "", tools: [
            { name: "read_file", description: "读取文件内容", writes: false },
            { name: "list_directory", description: "列出目录", writes: false },
            { name: "write_file", description: "写入或覆盖文件", writes: true },
          ] },
        } as Record<string, unknown>,
        mcpLogins: {} as Record<number, (result: { ok: boolean; message: string }) => void>,
        finishMcpLogin: (id: number, ok = true) => {
          if (ok) state.pi.mcp_servers = state.pi.mcp_servers.map((s) => s.id === id ? { ...s, logged_in: true } : s);
          harness.mcpLogins[id]?.({ ok, message: ok ? "" : "授权页已关闭" });
          delete harness.mcpLogins[id];
        },
        /** 推荐评测日志写入（record_*），供 e2e 断言曝光、行为与阅读会话。 */
        signals: { exposures: [] as any[], events: [] as any[], sessions: [] as any[] },
        repairBlocks: () => {
          for (const { raw, blocks } of brokenDocuments) { raw.__rrssMockBroken = false; raw.blocks = blocks; }
          brokenBlocks.reader = []; brokenBlocks.digest = []; brokenBlocks.messages = [];
        },
        holdOpen,
        releaseOpen: () => { for (const finish of pendingOpens.splice(0)) finish(); },
        holdFocusPage,
        releaseFocusPage: () => { for (const finish of pendingFocusPages.splice(0)) finish(); },
        holdDigest,
        releaseDigest: () => { for (const finish of pendingDigests.splice(0)) finish(); },
        emit: (event: string, payload: unknown) => {
          const web = payload as { label: string; url: string };
          if (event === "web:verify-state") {
            const result = payload as WebVerifyStateEvent;
            if (harness.webWindows[result.label] === result.url && result.state === "Extracted" && result.article_id != null) {
              const view = readerFor(result.article_id);
              readers[result.article_id] = { ...view, web_extracted_at: Date.now() };
            }
          }
          if (event === "web:closed") delete harness.webWindows[web.label];
          if (event === "web:navigated") harness.webWindows[web.label] = web.url;
          // web:escape 只转发给监听者；焦点由前端再调 set_web_view_visible(false)。
          for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload });
        },
        releaseSend: () => { for (const finish of pendingSends.splice(0)) finish(); },
        /** 对话引擎下载：`holdInstall` 时停在下载中，`progressInstall` 推进度，`finishInstall` 走完安装。 */
        holdInstall: false,
        progressInstall: (received: number) => {
          state.install = { ...state.install, state: "downloading", received };
          harness.emit("pi:install", { ...state.install });
        },
        finishInstall: () => {
          state.install = { ...state.install, state: "installing", received: state.install.total ?? 0 };
          harness.emit("pi:install", { ...state.install });
          state.probe = { ...state.probe, version: "pi 0.87.1", version_state: "ok" };
          state.install = { ...state.install, state: "done", message: "已下载 Pi 0.87.1" };
          harness.emit("pi:install", { ...state.install });
        },
        releaseClusterSummary: () => {
          for (const resolve of pendingClusterSummaries.splice(0)) resolve();
        },
        releaseSummary: () => {
          const text = summary ?? "";
          for (const resolve of pendingSummaries.splice(0)) resolve(text);
        },
        releaseSelectionTranslation: () => {
          for (const finish of pendingSelectionTranslations.splice(0)) finish();
        },
        state: undefined as unknown,
      };
      harness.state = state;
      const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
      // 模型与用途的模拟（阶段 19-7）：与宿主同样按手动设置 > 测试结果推出生效能力，并维护「用于哪些用途」。
      type Ref = { provider_id: number; model: string };
      const same = (a: Ref, b: Ref) => a.provider_id === b.provider_id && a.model === b.model;
      const effectiveOf = (e: PoolEntryView) => {
        const pick = (k: "tool_calling" | "structured_output" | "image_input" | "reasoning") =>
          e.overrides[k] === true ? "supported" as const : e.overrides[k] === false ? "unsupported" as const : e.detected[k];
        return { tool_calling: pick("tool_calling"), structured_output: pick("structured_output"), image_input: pick("image_input"), reasoning: pick("reasoning") };
      };
      // 订阅时间线、计数和按范围标已读共用一份文章。文件夹子孙按 feed_tree 的缩进前序。
      let feedsTimeline = timelineArticles ?? null;
      const feedsSource = () => (feedsTimeline ?? state.focus.articles).filter((row) => row.row_kind === 0);
      const sortAt = (row: ArticleView) => row.published_at ?? 0;
      const scopeFeedIds = (scope: string): { ids: Set<number> | null; scope: string } => {
        const listed = [...state.feeds.folders.flatMap((folder) => folder.feeds), ...state.feeds.uncategorized];
        const feed = /^feed:(-?\d+)$/.exec(scope);
        if (feed) {
          const id = Number(feed[1]);
          return listed.some((item) => item.id === id) ? { ids: new Set([id]), scope } : { ids: null, scope: "all" };
        }
        const folderMatch = /^folder:(-?\d+)$/.exec(scope);
        if (!folderMatch) return { ids: null, scope: "all" };
        const folders = state.feeds.folders;
        const index = folders.findIndex((item) => item.id === Number(folderMatch[1]));
        if (index < 0) return { ids: null, scope: "all" };
        const base = folders[index].indent;
        const ids = new Set<number>();
        for (let i = index; i < folders.length; i++) {
          if (i > index && folders[i].indent <= base) break;
          for (const item of folders[i].feeds) ids.add(item.id);
        }
        return { ids, scope };
      };
      const syncModels = () => {
        state.pool = { ...state.pool, pool: state.pool.pool.map((e) => {
          const next = { ...e, effective: effectiveOf(e) };
          return { ...next, uses: state.bindings.tasks.filter((t) => same(t, e)).map((t) => t.task) };
        }) };
        state.bindings = { ...state.bindings, tasks: state.bindings.tasks.map((t) => {
          const e = state.pool.pool.find((x) => same(x, t));
          return { ...t, capabilities: e?.effective };
        }) };
      };
      Object.assign(window, {
        __mockHost: harness,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (name: string, id: number) => listeners.get(name)?.delete(id) },
        __TAURI_INTERNALS__: {
          metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
          transformCallback(cb: (event: unknown) => void) {
            callbacks.set(++callbackId, cb);
            return callbackId;
          },
          async invoke(command: string, args: Record<string, any> = {}) {
            harness.calls.push(command);
            harness.args[command] = args;
            switch (command) {
              case "plugin:event|listen": {
                const ids = listeners.get(args.event) ?? new Set<number>();
                ids.add(args.handler);
                listeners.set(args.event, ids);
                return args.handler;
              }
              case "plugin:event|unlisten":
                return;
              case "focus_stream": {
                if (harness.holdFocusPage) await new Promise<void>(resolve => pendingFocusPages.push(resolve));
                const iconsOn = state.settings.values.find(v => v.id === "feed_icons_enabled")?.value !== "0";
                const scope = state.focus.list_scope;
                const requested = String(args.scope ?? "");
                const column = requested.startsWith("folder:") ? scopeFeedIds(requested) : { ids: null, scope: "all" };
                // 与 articles::project_clusters 相同：展开的组只给标题行和成员，紧跟标题行的代表不再单列一行。
                const members = clusterArticles[args.expandedCluster] ?? [];
                const expanded: ArticleView[] = [];
                let skipping = false;
                for (const row of state.focus.articles) {
                  if (row.row_kind === 1 && row.cluster_id === args.expandedCluster) {
                    expanded.push(row, ...members);
                    skipping = members.length > 0;
                    continue;
                  }
                  if (skipping && row.row_kind === 0 && row.cluster_id === args.expandedCluster) continue;
                  skipping = false;
                  expanded.push(row);
                }
                const rows = expanded.filter((row) => {
                  if (column.ids !== null && !column.ids.has(row.feed_id)) return false;
                  if (row.row_kind !== 0) return scope === "all";
                  if (scope === "starred") return row.starred;
                  if (scope === "unread") return !row.read;
                  return true;
                });
                const remaining = rows.filter(row => !(args.excludeIds ?? []).includes(row.id));
                const articles = (focusPageSize ? remaining.slice(0, focusPageSize) : rows).map(row => ({ ...row, feed_icon_href: iconsOn ? feedIconHrefs[row.feed_id] ?? row.feed_icon_href : "" }));
                const scoped = column.ids !== null;
                return { ...state.focus, articles,
                  digest_banner: scoped ? "" : state.focus.digest_banner,
                  digest_lede: scoped ? "" : state.focus.digest_lede,
                  has_more: focusPageSize ? remaining.length > focusPageSize : state.focus.has_more };
              }
              case "focus_cluster_members": {
                const requested = String(args.scope ?? "");
                const column = requested.startsWith("folder:") ? scopeFeedIds(requested) : { ids: null, scope: "all" };
                return [...(clusterArticles[args.clusterId] ?? [])].filter((row) => column.ids === null || column.ids.has(row.feed_id));
              }
              case "feeds_page": {
                const resolved = scopeFeedIds(String(args.scope ?? ""));
                const listScope = args.listScope;
                const ranked = feedsSource().filter((row) => resolved.ids === null || resolved.ids.has(row.feed_id))
                  .filter((row) => listScope === "unread" ? !row.read : listScope === "starred" ? !!row.starred : true)
                  .slice().sort((a, b) => sortAt(b) - sortAt(a) || b.id - a.id);
                const cursor = args.cursor as { at: number; id: number } | null;
                const after = cursor ? ranked.filter((row) => sortAt(row) < cursor.at || (sortAt(row) === cursor.at && row.id < cursor.id)) : ranked;
                const articles = after.slice(0, feedsPageSize);
                const last = articles[articles.length - 1];
                const hasMore = after.length > feedsPageSize;
                return { articles, has_more: hasMore, next_cursor: hasMore && last ? { at: sortAt(last), id: last.id } : null, hidden_count: feedsHiddenCount, scope: resolved.scope };
              }
              case "feed_counts": {
                const unread = new Map<number, number>();
                for (const row of feedsSource()) if (!row.read) unread.set(row.feed_id, (unread.get(row.feed_id) ?? 0) + 1);
                const rolled = new Map<number, number>();
                const folders = state.feeds.folders;
                for (const [feedId, count] of unread) {
                  const index = folders.findIndex((folder) => folder.feeds.some((feed) => feed.id === feedId));
                  if (index < 0) continue;
                  let indent = folders[index].indent;
                  rolled.set(folders[index].id, (rolled.get(folders[index].id) ?? 0) + count);
                  for (let i = index - 1; i >= 0 && indent > 0; i--) {
                    if (folders[i].indent < indent) {
                      rolled.set(folders[i].id, (rolled.get(folders[i].id) ?? 0) + count);
                      indent = folders[i].indent;
                    }
                  }
                }
                const counts = (rows: Map<number, number>) => [...rows].sort((a, b) => a[0] - b[0]).map(([id, n]) => ({ id, unread: n }));
                return { total: [...unread.values()].reduce((sum, n) => sum + n, 0), feeds: counts(unread), folders: counts(rolled) };
              }
              case "cluster_summary": {
                if (clusterSummaries[args.clusterId]) return clusterSummaries[args.clusterId];
                // 还没生成：列组内文章，一句话取摘要。
                const members = (clusterArticles[args.clusterId] ?? []).map((row) => ({ article_id: row.id, feed_id: row.feed_id,
                  feed_title: row.feed_title, icon_href: row.feed_icon_href, title: row.title, published_at: row.published_at, one_liner: row.summary }));
                return { cluster_id: args.clusterId, state: "none", model_ready: clusterSummaryModelReady, estimate_tokens: 2000, overview: "",
                  additions: [], conflicts: [], basis: 0, generated_at: null, fresh: [], members };
              }
              case "generate_cluster_summary": {
                const result = clusterGenerated[args.clusterId];
                if (!result) throw new Error(`mock host: no generated summary for cluster ${args.clusterId}`);
                if (holdClusterSummary) await new Promise<void>((resolve) => { pendingClusterSummaries.push(resolve); });
                clusterSummaries[args.clusterId] = result;
                return result;
              }
              case "mark_cluster_read": {
                // 与 articles::mark_cluster_read 相同：只标这组在范围内规则可见的未读，范围同 focus_cluster_members。
                const requested = String(args.scope ?? "");
                const column = requested.startsWith("folder:") ? scopeFeedIds(requested) : { ids: null, scope: "all" };
                const threshold = state.focus.ranking.hide_threshold;
                const marked: number[] = [];
                const members = (clusterArticles[args.clusterId] ?? []).map((row) => {
                  if (row.read || row.score <= threshold || (column.ids !== null && !column.ids.has(row.feed_id))) return row;
                  marked.push(row.id);
                  return { ...row, read: true };
                });
                clusterArticles[args.clusterId] = members;
                const done = new Set(marked);
                state.focus = { ...state.focus, articles: state.focus.articles.map((row) => row.row_kind === 0 && done.has(row.id) ? { ...row, read: true } : row) };
                return marked.sort((a, b) => a - b);
              }
              case "mark_scope_read": {
                const { ids } = scopeFeedIds(String(args.scope ?? ""));
                const threshold = state.focus.ranking.hide_threshold;
                const source = feedsTimeline ?? state.focus.articles;
                const marked: number[] = [];
                const next = source.map((row) => {
                  // 与 timeline::mark_scope_read 相同：只标范围内规则可见的未读。
                  if (row.row_kind !== 0 || row.read || row.score <= threshold || (ids !== null && !ids.has(row.feed_id))) return row;
                  marked.push(row.id);
                  return { ...row, read: true };
                });
                if (feedsTimeline) feedsTimeline = next;
                else state.focus = { ...state.focus, articles: next };
                return marked;
              }
              case "feed_tree": {
                const iconsOn = state.settings.values.find(v => v.id === "feed_icons_enabled")?.value !== "0";
                const icon = (feed: typeof feeds.uncategorized[number]) => ({ ...feed, icon_href: iconsOn ? feedIconHrefs[feed.id] ?? feed.icon_href : "" });
                return { ...state.feeds, folders: state.feeds.folders.map(folder => ({ ...folder, feeds: folder.feeds.map(icon) })), uncategorized: state.feeds.uncategorized.map(icon) };
              }
              case "threads":
                return structuredClone(state.threads.filter(t => t.archived_at == null));
              // 会话文件夹与归档：只保留命令契约与写后可读，排序、标题回退与删除锁在后端测试。
              case "thread_list": {
                const live = state.threads.filter(t => t.archived_at == null)
                  .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updated_at - a.updated_at || b.id - a.id);
                return structuredClone({ folders: [...state.threadFolders].sort((a, b) => a.position - b.position), threads: live,
                  archived_count: state.threads.length - live.length });
              }
              case "archived_threads":
                return structuredClone(state.threads.filter(t => t.archived_at != null).sort((a, b) => (b.archived_at ?? 0) - (a.archived_at ?? 0)));
              case "rename_thread":
              case "set_thread_folder":
              case "set_thread_pinned":
              case "archive_thread":
              case "unarchive_thread": {
                const thread = state.threads.find(t => t.id === args.threadId);
                if (!thread) throw new Error("会话不存在");
                if (command === "rename_thread") thread.title = String(args.title).trim() || (messages[thread.id]?.[0]?.text ?? "新对话").slice(0, 20);
                if (command === "set_thread_folder") thread.folder_id = args.folderId ?? null;
                if (command === "set_thread_pinned") thread.pinned = !!args.pinned;
                if (command === "archive_thread") thread.archived_at = Date.now();
                if (command === "unarchive_thread") thread.archived_at = null;
                return null;
              }
              case "delete_thread":
                state.threads = state.threads.filter(t => t.id !== args.threadId);
                return null;
              case "create_thread_folder": {
                const folder = { id: Math.max(0, ...state.threadFolders.map(f => f.id)) + 1, name: String(args.name).trim(),
                  position: state.threadFolders.length, collapsed: false };
                state.threadFolders = [...state.threadFolders, folder];
                return structuredClone(folder);
              }
              case "rename_thread_folder":
                state.threadFolders = state.threadFolders.map(f => f.id === args.folderId ? { ...f, name: String(args.name).trim() } : f);
                return null;
              case "delete_thread_folder":
                state.threadFolders = state.threadFolders.filter(f => f.id !== args.folderId);
                state.threads.forEach(t => { if (t.folder_id === args.folderId) t.folder_id = null; });
                return null;
              case "reorder_thread_folders":
                state.threadFolders = state.threadFolders.map(f => ({ ...f, position: (args.ids as number[]).indexOf(f.id) }));
                return null;
              case "set_thread_folder_collapsed":
                state.threadFolders = state.threadFolders.map(f => f.id === args.folderId ? { ...f, collapsed: !!args.collapsed } : f);
                return null;
              case "search_threads": {
                const q = String(args.query).trim().toLowerCase();
                if (!q) return [];
                return state.threads.filter(t => t.archived_at == null).flatMap(t => {
                  const hit = (messages[t.id] ?? []).find((m: { text: string }) => m.text.toLowerCase().includes(q));
                  const titleMatch = t.title.toLowerCase().includes(q);
                  if (!hit && !titleMatch) return [];
                  return [{ thread_id: t.id, title: t.title, folder_id: t.folder_id, message_id: hit?.id ?? null, role: hit?.role ?? "assistant",
                    snippet: hit?.text ?? "", created_at: t.updated_at, updated_at: t.updated_at, title_match: titleMatch }];
                }).slice(0, args.limit ?? 20);
              }
              case "messages": {
                const all = (messages[args.threadId] ?? []).filter((m: { id: number }) => args.beforeId == null || m.id < args.beforeId);
                const window = all.slice(Math.max(0, all.length - 40));
                return { messages: window.map(messageFor), has_more: all.length > 40 };
              }
              case "digest_list":
                if (harness.holdDigest) await new Promise<void>(resolve => pendingDigests.push(resolve));
                return digests.filter((d) => d.period === args.period).map((d) => ({ id: d.id, period: d.period, period_label: d.period_label, period_start: d.period_start, date_label: d.date_label }));
              case "settings_schema":
                return structuredClone(state.settings);
              case "set_setting": {
                // 保留命令契约与写后可读；业务校验在后端测试，失败由场景注入。
                const spec = state.settings.specs.find((s: { id: string }) => s.id === args.id);
                if (!spec) throw new Error(`没有名为 ${args.id} 的设置项`);
                const raw = String(args.value).trim();
                state.settings.values = state.settings.values.map((v: { id: string; value: string }) => (v.id === args.id ? { id: args.id, value: raw } : v));
                if (args.id === "theme_preference") {
                  state.theme = raw;
                  state.focus = { ...state.focus, ranking: { ...state.focus.ranking, theme: raw } };
                }
                if (args.id === "focus_filter") state.focus = { ...state.focus, filter: raw };
                if (args.id === "list_scope") state.focus = { ...state.focus, list_scope: raw };
                const rankingKey = { cluster_group_enabled: "cluster_group", interest_boost_enabled: "interest_boost",
                  focus_show_details: "show_details", focus_rerank_enabled: "rerank", focus_hide_noisy: "hide_noisy", rank_features_enabled: "rank_features",
                  rank_source_enabled: "rank_source", rank_coverage_enabled: "rank_coverage", rank_freshness_enabled: "rank_freshness", rank_affinity_enabled: "rank_affinity" }[args.id as string];
                if (rankingKey) state.focus = { ...state.focus, ranking: { ...state.focus.ranking, [rankingKey]: raw === "1" } };
                if (args.id === "hide_threshold") {
                  state.rules = { ...state.rules, hide_threshold: Number(raw) };
                  state.focus = { ...state.focus, ranking: { ...state.focus.ranking, hide_threshold: Number(raw) } };
                }
                // 打开自动摘要时后端顺手把升级提示标为看过。
                if (args.id === "focus_auto_summary" && raw === "1") state.ai = { ...state.ai, auto_summary: { ...state.ai.auto_summary, notice: null } };
                if (args.id === "developer_tools") state.pi = { ...state.pi, developer_tools: raw === "1" };
                if (args.id === "fetch_url_enabled") state.pi = { ...state.pi, fetch_url: raw === "1" };
                if (args.id === "translate_target_lang") state.translate = { ...state.translate, target_lang: raw };
                return { id: args.id, value: raw };
              }
              case "digest": {
                if (harness.holdDigest) await new Promise<void>(resolve => pendingDigests.push(resolve));
                const row = digests.find((d) => d.id === args.id);
                if (!row) throw new Error("digest not found");
                return brokenBlocks?.digest?.includes(row.id) ? { ...row, document: brokenDocument(row.document) } : row;
              }
              case "article_previews": {
                // 与宿主一致：按请求顺序，去重、忽略非正 id；库里没有的标 removed。
                const ids = [...new Set((args.ids as number[]).filter((id) => id > 0))];
                return ids.map((id) => {
                  const row = state.focus.articles.find((article) => article.id === id);
                  return row
                    ? { article_id: id, title: row.title, feed_title: row.feed_title, summary: row.summary || null, published_at: row.published_at, read: row.read, url: row.url, status: "available" }
                    : { article_id: id, title: "", feed_title: "", summary: null, published_at: null, read: false, url: "", status: "removed" };
                });
              }
              case "digest_thread": {
                const id = digestThreads.get(args.digestId) ?? 0;
                return { thread_id: id, chat_model: state.threads.find(t => t.id === id)?.chat_model ?? null, chat_thinking: state.threads.find(t => t.id === id)?.chat_thinking ?? null, context: state.threads.find(t => t.id === id)?.context ?? null, messages: (messages[id] ?? []).map(messageFor), has_more: false };
              }
              case "send_message": {
                if (holdSend) await new Promise<void>(resolve => pendingSends.push(resolve));
                if (remainingSendFailures > 0) { --remainingSendFailures; throw new Error("模拟发送失败，请重试"); }
                const input = args.args;
                const id = input.threadId || (input.digestId ? digestThreads.get(input.digestId) : 0) || nextThreadId++;
                if (input.digestId) digestThreads.set(input.digestId, id);
                const thread = state.threads.find(t => t.id === id) ?? { id, title: input.text || "针对引用提问", chat_model: null, chat_thinking: null, context: null, folder_id: null, pinned: false, updated_at: Date.now(), archived_at: null };
                thread.chat_model = input.model ?? null;
                if (input.thinking != null) thread.chat_thinking = input.thinking || null;
                const selected = thread.chat_model ?? state.bindings.tasks.find(t => t.task === "chat");
                const entry = state.pool.pool.find(e => e.provider_id === selected?.provider_id && e.model === selected?.model);
                const window = entry?.context_window ?? entry?.engine_context_window ?? 32000;
                if (thread.context) thread.context = { ...thread.context, window };
                // 在归档的会话里发送新消息时宿主自动取消归档。
                thread.archived_at = null;
                thread.updated_at = Date.now();
                state.threads = [thread, ...state.threads.filter(t => t.id !== id)];
                const message = (role: string, text: string, citations: CitationView[] = []) => ({ id: nextMessageId++, role, text, citations, document: null, streaming: false, cite_article_id: 0, cite_title: "" });
                messages[id] = [...(messages[id] ?? []), message("user", input.text, input.citations)];
                harness.emit("chat:start", id);
                const answer = "我会结合这些引用继续分析。";
                messages[id].push(message("assistant", answer));
                harness.emit("chat:delta", { thread_id: id, text: answer, document: null });
                thread.context = { used: Math.ceil(messages[id].reduce((sum,m) => sum + m.text.length,0) / 3), window, at: Date.now() };
                harness.emit("chat:context", { thread_id: id, context: thread.context });
                harness.emit("chat:done", id);
                return id;
              }
              case "translate_snapshot":
                return { kind: "missing", reader: readerFor(args.id), snapshot: null, note: "", reconcile_after_ms: 45000 };
              case "translate_attach":
              case "translate_detach":
              case "translate_hint":
                return;
              case "site_rules_page": return structuredClone(siteRules);
              case "save_site_rule": {
                const rule = structuredClone(args.rule);
                rule.domain = rule.domain.trim().toLowerCase().replace(/^www\./, "");
                siteRules.users = siteRules.users.filter(v => v.rule.domain !== rule.domain);
                siteRules.users.push({ rule, enabled: args.enabled, updated_at: Date.now() });
                return;
              }
              case "delete_site_rule": siteRules.users = siteRules.users.filter(v => v.rule.domain !== args.domain); return;
              case "set_site_rule_enabled": {
                const row = siteRules.users.find(v => v.rule.domain === args.domain);
                if (row) row.enabled = args.enabled;
                return;
              }
              case "test_site_rule":
                if (!siteRuleReport) throw new Error("mock host: missing site rule report");
                return structuredClone(siteRuleReport);
              case "open_web_view": {
                if (harness.webWindowError) throw harness.webWindowError;
                harness.webViewBounds.push(args.bounds);
                for (const label of Object.keys(harness.webWindows)) harness.emit("web:closed", { label });
                const label = `web-${++harness.nextWebWindow}`;
                harness.webWindows[label] = args.url;
                return label;
              }
              case "extract_web_view": {
                const url = harness.webWindows[args.label];
                if (!url) throw new Error("原网页已关闭");
                const result = webExtractions[url];
                if (!result) throw new Error("mock host: missing web extraction fixture");
                const payload = { ...result, label: args.label, url };
                harness.emit("web:verify-state", { ...payload, state: "PreExtract", article_id: null });
                harness.emit("web:verify-state", payload);
                return structuredClone(payload);
              }
              case "set_web_view_bounds":
                harness.webViewBounds.push(args);
                return;
              case "set_web_view_visible":
                harness.webViewVisible.push({ label: String(args.label), visible: args.visible === true });
                return;
              case "web_view_action":
                // 关闭与清空在真实宿主里会销毁子 webview 并发 web:closed。
                if (args.action === "close" || args.action === "clear") {
                  for (const label of Object.keys(harness.webWindows)) harness.emit("web:closed", { label });
                }
                return;
              case "close_web_view":
                if (!harness.webWindows[args.label]) throw new Error("原网页已关闭");
                // 销毁由用例经真实 listen 契约显式推送 web:closed。
                return;
              case "clear_web_data":
                if (harness.clearWebDataError) throw harness.clearWebDataError;
                for (const label of Object.keys(harness.webWindows)) harness.emit("web:closed", { label });
                harness.webWindows = {};
                return;
              case "open_web_link": {
                if (harness.holdOpen) await new Promise<void>(resolve => pendingOpens.push(resolve));
                const result = webLinks[args.url];
                if (!result) throw new Error("mock host: missing web link fixture");
                if ("kind" in result) throw result;
                return structuredClone(result);
              }
              case "collect_article":
                collectWebArticle(args.id);
                return;
              case "plugin:opener|open_url":
                return;
              case "window_chrome":
                return { controls: windowControls, tray: true };
              case "tray_install":
                harness.window.trayLabels = args.labels;
                return true;
              case "window_close_choice": {
                harness.window.closeChoices.push({ choice: args.choice, remember: args.remember });
                if (args.remember) state.settings.values = state.settings.values.map((v: { id: string; value: string }) => (v.id === "close_action" ? { id: v.id, value: args.choice } : v));
                if (args.choice === "tray") harness.window.hidden = true;
                else harness.window.closed = true;
                return;
              }
              case "plugin:window|minimize":
                harness.window.minimized += 1;
                return;
              case "plugin:window|toggle_maximize":
                harness.window.maximized = !harness.window.maximized;
                harness.emit("tauri://resize", { width: 1280, height: 800 });
                return;
              case "plugin:window|is_maximized":
                return harness.window.maximized;
              case "plugin:window|close": {
                // 宿主拦下关闭后按设置「关闭窗口时」处理（后台驻留）。
                const action = state.settings.values.find((v: { id: string }) => v.id === "close_action")?.value ?? "ask";
                if (action === "ask") harness.emit("window:close-requested", null);
                else if (action === "tray") harness.window.hidden = true;
                else harness.window.closed = true;
                return;
              }
              case "link_safety":
                return structuredClone(linkSafety[args.href] ?? { actual_host: new URL(args.href).hostname, warnings: [] });
              case "article_reader":
                return readerFor(args.id);
              case "related_articles":
                return structuredClone((related[args.articleId] ?? []).slice(0, args.limit));
              case "open_article": {
                if (harness.holdOpen) await new Promise<void>(resolve => pendingOpens.push(resolve));
                const sequence = readerSequences[args.id];
                if (sequence?.length) readers[args.id] = sequence.length > 1 ? sequence.shift()! : sequence[0];
                state.focus = {
                  ...state.focus,
                  articles: state.focus.articles.map((row) => (row.id === args.id ? { ...row, read: true } : row)),
                };
                return readerFor(args.id);
              }
              case "prefetch_images":
                return [];
              case "prefetch_embed_covers":
                return;
              case "download_direct_media":
                return { src: args.url, media_id: 1, href: "rrss://localhost/media/1", width: null, height: null, error: "" };
              case "retry_image":
                return { src: args.url, media_id: 1, href: "rrss://localhost/media/1", width: null, height: null, error: "" };
              case "search_articles": {
                const options = args.query as import("../src/bindings").SearchQuery;
                const q = normalizeSearch(options.query);
                if (!q) return [];
                const results = state.focus.articles
                  .filter(a => a.row_kind === 0 && a.score > state.rules.hide_threshold
                    && (options.scope !== "unread" || !a.read) && (options.scope !== "starred" || a.starred)
                    && (!options.feed_ids?.length || options.feed_ids.includes(a.feed_id))
                    && (options.published_since == null || (a.published_at != null && a.published_at >= options.published_since)))
                  .map(a => { const body = searchBody(a.id); return {
                    id: a.id, title: a.title, feed_title: a.feed_title, feed_icon_href: iconFor(a.feed_id), published_at: a.published_at, score: a.score,
                    read: a.read, starred: a.starred, title_match: matchesSearch(a.title, q), body_match: matchesSearch(body, q), snippet: searchSnippet(body, q),
                  }; })
                  .filter(a => a.title_match || a.body_match);
                const limit = options.limit ?? 20;
                return limit < 0 ? results : results.slice(0, limit);
              }
              case "search_hit_blocks":
                return searchBlocks(args.articleId, args.query);
              case "refresh": {
                const id = (refreshOperation?.id ?? 0) + 1;
                refreshOperation = { id, target: 1, completed: 1, inserted: refreshFails ? 2 : 3,
                  failed: refreshFails ? 1 : 0, failed_feeds: refreshFails ? [1] : [], deleted: 0, skipped: 0, done: true, error: null };
                await wait(120);
                return { run_id: "mock", operation_id: id };
              }
              case "scheduler_status":
                return { run_id: "mock", version: refreshOperation?.id ?? 0, operations: refreshOperation ? [refreshOperation] : [],
                  background_requested: 0, background_completed: 0, background: [], stopped: false, feed_pause: null };
              case "ai_page":
                return structuredClone({ ...state.ai, selected_id: args.selectedId || state.ai.selected_id });
              case "dismiss_auto_summary_notice":
                state.ai = { ...state.ai, auto_summary: { ...state.ai.auto_summary, notice: null } };
                return null;
              case "provider_presets":
                return structuredClone(presets);
              case "set_vector_dims":
              case "cancel_vector_rebuild":
              case "check_vector_space":
              case "restore_vector_space":
              case "vector_space_status": {
                const v = state.vectors;
                if (command === "set_vector_dims") {
                  v.dims_setting = args.value;
                  if (v.effective) {
                    const source = v.effective.source_dims || v.effective.dims;
                    const dims = args.value === "native" ? source : Math.min(args.value === "auto" ? 1024 : Number(args.value), source);
                    if (dims !== v.effective.dims) v.building = {
                      space: { ...v.effective, id: v.effective.id + 1, dims, state: "building", chunks: 0 },
                      written: 0, total: v.estimate_chunks,
                    };
                  }
                }
                if (command === "cancel_vector_rebuild") {
                  if (v.effective && v.building?.space.model === v.effective.model && v.building.space.dims !== v.effective.dims) {
                    v.dims_setting = v.effective.dims === v.effective.source_dims ? "native" : String(v.effective.dims);
                  }
                  v.building = null;
                }
                if (command === "restore_vector_space" && v.effective) v.effective = { ...v.effective, state: "active", stale_reason: null };
                if (command !== "vector_space_status" && v.effective) v.effective = { ...v.effective, checked_at: Date.now() };
                return structuredClone({ ...v, allow_model_change: state.settings.values.find(x => x.id === "allow_embedding_model_change")?.value === "1" });
              }
              case "model_bindings":
                return structuredClone(state.bindings);
              case "save_model_bindings": {
                const next = args.args as { bindings: Array<Ref & { task: string }> };
                for (const b of next.bindings) {
                  if (b.provider_id && !state.pool.pool.some((e) => same(e, b) && (b.task !== "chat" || e.chat))) throw new Error(`${b.model} 还没有加进模型列表`);
                }
                state.bindings = { ...state.bindings, tasks: state.bindings.tasks.map((t) => {
                  const b = next.bindings.find((x) => x.task === t.task);
                  return b ? { ...t, provider_id: b.provider_id, model: b.model } : t;
                }) };
                syncModels();
                return structuredClone(state.bindings);
              }
              case "model_pool":
                return structuredClone(state.pool);
              case "save_model_pool": {
                const { add, remove, replace } = args.args as { add: Ref[]; remove: Ref[]; replace?: Ref };
                const kept = state.pool.pool.filter((e) => !remove.some((r) => same(r, e)));
                let chatEmpty = !kept.some((e) => e.chat);
                const added = add.filter((a) => !kept.some((e) => same(a, e))).map((a) => {
                  const chat = chatEmpty && !/embed/i.test(a.model);
                  if (chat) chatEmpty = false;
                  const unknown = "unknown" as const;
                  const detected = { tool_calling: unknown, structured_output: unknown, image_input: unknown, reasoning: unknown };
                  return { ...a, chat, display_name: "", thinking: "off", context_window: undefined, engine_context_window: undefined, detected, overrides: {}, effective: detected, uses: [] as string[] };
                });
                state.pool = { ...state.pool, pool: [...kept, ...added] };
                const target = replace && state.pool.pool.find((e) => same(e, replace));
                const replaced: string[] = [];
                const unset: string[] = [];
                state.bindings = { ...state.bindings, tasks: state.bindings.tasks.map((t) => {
                  if (!remove.some((r) => same(r, t))) return t;
                  if (target && (t.task !== "chat" || target.chat)) { replaced.push(t.task); return { ...t, provider_id: target.provider_id, model: target.model }; }
                  unset.push(t.task);
                  return { ...t, provider_id: 0, model: "" };
                }) };
                syncModels();
                return { ...structuredClone(state.pool), replaced, unset };
              }
              case "update_pool_model": {
                const next = args.args as Ref & { chat?: boolean; display_name?: string; thinking?: string; context_window?: number;
                  max_output?: number; compat_json?: string; overrides?: PoolEntryView["overrides"] };
                const chat = state.bindings.tasks.find((t) => t.task === "chat");
                if (next.chat === false && chat && same(chat, next)) throw new Error("它是对话默认模型，先在模型页「对话」里改选别的，再关掉");
                state.pool = { ...state.pool, pool: state.pool.pool.map((e) => !same(e, next) ? e : {
                  ...e,
                  chat: next.chat ?? e.chat,
                  display_name: next.display_name ?? e.display_name,
                  thinking: next.thinking ?? e.thinking,
                  context_window: next.context_window === undefined ? e.context_window : next.context_window || undefined,
                  max_output: next.max_output === undefined ? e.max_output : next.max_output || undefined,
                  compat_json: next.compat_json === undefined ? e.compat_json : next.compat_json.trim() ? next.compat_json : undefined,
                  overrides: next.overrides ?? e.overrides,
                }) };
                syncModels();
                return structuredClone(state.pool);
              }
              case "probe_pool_model": {
                await wait(60);
                state.pool = { ...state.pool, pool: state.pool.pool.map((e) => !same(e, args as Ref) ? e : {
                  ...e, tested_at: Date.now(),
                  detected: { tool_calling: "supported", structured_output: "supported", image_input: e.detected.image_input, reasoning: "unknown" },
                }) };
                syncModels();
                return structuredClone(state.pool);
              }
              case "fetch_model_catalog": {
                await wait(60);
                const models = ["gpt-4o-mini", "qwen-max", "text-embedding-3-small"];
                const catalogs = state.pool.catalogs.filter((c) => c.provider_id !== args.providerId);
                state.pool = { ...state.pool, catalogs: [...catalogs, { provider_id: args.providerId, models, fetched_at: Date.now() }] };
                return structuredClone(state.pool);
              }
              case "save_ai_settings": {
                const input = args.args.provider;
                const id = input.id || Math.max(0, ...state.ai.providers.map((p) => p.id)) + 1;
                const old = state.ai.providers.find((p) => p.id === id);
                const key = input.api_key?.trim();
                const saved = { id, kind: input.kind ?? old?.kind ?? "openai_compat", name: input.name, base_url: input.base_url, protocol: input.protocol,
                  key: key ? "set" as const : input.clear_key ? "none" as const : old?.key ?? "none" as const,
                  key_hint: key ? keyHint(key) : input.clear_key ? "" : old?.key_hint ?? "",
                  last_test: old?.last_test, compat_json: input.compat_json ?? old?.compat_json ?? "{}" };
                state.ai = { ...state.ai, selected_id: id, providers: old
                  ? state.ai.providers.map((p) => (p.id === id ? saved : p)) : [...state.ai.providers, saved] };
                return structuredClone(state.ai);
              }
              case "delete_provider":
                state.ai = { ...state.ai, providers: state.ai.providers.filter((p) => p.id !== args.id) };
                state.pool = { ...state.pool, pool: state.pool.pool.filter((e) => e.provider_id !== args.id),
                  catalogs: state.pool.catalogs.filter((c) => c.provider_id !== args.id) };
                state.bindings = { ...state.bindings, tasks: state.bindings.tasks.map((t) => t.provider_id === args.id ? { ...t, provider_id: 0, model: "" } : t) };
                syncModels();
                return;
              case "test_connection": {
                await wait(60);
                const input = args.args as { provider_id: number; base_url: string; api_key?: string };
                const result = { ok: true, latency_ms: 42, models: 7, error: "" };
                if (input.provider_id && !input.api_key) {
                  state.ai = { ...state.ai, providers: state.ai.providers.map((p) => p.id === input.provider_id
                    ? { ...p, last_test: { at: Date.now(), ok: true, note: "7" } } : p) };
                }
                return result;
              }
              case "rule_stats":
                return structuredClone(ruleStats ?? { rules: [], spot_check: { exposed: 0, liked: 0, disliked: 0, insufficient: true } });
              case "hidden_page": {
                const page = structuredClone(hidden ?? { articles: [], rule_counts: [], spot_liked_count: 0 });
                if (args.scope == null) return page;
                const { ids } = scopeFeedIds(String(args.scope));
                if (ids === null) return page;
                page.articles = page.articles.filter((article) => ids.has(article.feed_id));
                page.spot_liked_count = page.articles.filter((article) => article.spot_liked).length;
                return page;
              }
              case "rules_page":
                return structuredClone(state.rules);
              case "memory_page":
                return structuredClone(state.memory);
              case "pi_page":
                if (piDelayMs) await wait(piDelayMs);
                return structuredClone(state.pi);
              case "pi_probe":
                if (piDelayMs) await wait(piDelayMs);
                return structuredClone(state.probe);
              case "restart_pi":
                await wait(80);
                return;
              case "upsert_mcp": {
                const input = args.input;
                const old = state.pi.mcp_servers.find((s) => s.id === input.id);
                const id = old?.id ?? Math.max(0, ...state.pi.mcp_servers.map((s) => s.id)) + 1;
                const secrets = (key: "env" | "headers") => input[key].map((e: { name: string; value: string }) => ({ name: e.name,
                  has_value: !!e.value || !!old?.[key].find((s) => s.name === e.name)?.has_value }));
                const saved = { ...input, id, logged_in: old?.logged_in ?? false, enabled: input.transport !== "sse" && (old?.enabled ?? false), env: secrets("env"), headers: secrets("headers") };
                state.pi.mcp_servers = [...state.pi.mcp_servers.filter((s) => s.id !== id), saved];
                return id;
              }
              case "toggle_mcp_server":
                state.pi.mcp_servers = state.pi.mcp_servers.map((s) => s.id === args.id ? { ...s, enabled: args.enabled } : s);
                return;
              case "delete_mcp_server":
                state.pi.mcp_servers = state.pi.mcp_servers.filter((s) => s.id !== args.id);
                return;
              case "save_pi_settings":
                state.pi = { ...state.pi, pi_path: args.args.pi_path.trim() };
                return structuredClone(state.pi);
              case "pi_config":
                return structuredClone(state.piConfig);
              case "save_pi_config": {
                const next = { ...state.piConfig };
                for (const key of ["settings_override", "models_override"] as const) {
                  const raw = args.args[key];
                  if (raw === undefined) continue;
                  const value = raw.trim() ? JSON.parse(raw) : {};
                  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("覆盖必须是 JSON 对象");
                  next[key] = JSON.stringify(value);
                }
                for (const key of ["use_own_config", "own_model"] as const) if (args.args[key] !== undefined) (next as any)[key] = args.args[key];
                state.piConfig = next;
                state.bindings = { ...state.bindings, chat_uses_own_config: next.use_own_config };
                return structuredClone(next);
              }
              case "network_proxy":
                return structuredClone(state.networkProxy);
              case "save_network_proxy": {
                // 与后端同义：各规则（含非活动规则）都保留地址；认证只在填写或清除时改变，回传只有 has_auth。
                const input = args.args as SaveNetworkProxyArgs;
                const next = structuredClone(state.networkProxy);
                for (const key of ["global", "content", "api", "pi"] as const) {
                  const rule = input[key];
                  const url = rule.url.trim();
                  if (rule.mode === "custom" && !/^(https?|socks5h?):\/\/[^/\s]+$/i.test(url.replace(/\/$/, ""))) throw new Error("代理地址格式无效");
                  const auth = rule.clear_auth ? false : rule.username !== undefined || rule.password !== undefined || next[key].has_auth;
                  // 等价 url crate：http / https 补结尾斜杠，socks5 / socks5h 保留原样。
                  next[key] = { mode: rule.mode, url: /^https?:\/\/[^/]+$/i.test(url) ? `${url}/` : url, no_proxy: rule.no_proxy, has_auth: !!url && auth };
                }
                state.networkProxy = next;
                return structuredClone(state.networkProxy);
              }
              case "test_network_proxy": {
                // 不联网：按草稿里该用途的实际线路给结果，地址含 unreachable 的自定义代理视为连不上。
                const { scope, draft } = args.args as { scope: "content" | "api" | "pi"; draft: SaveNetworkProxyArgs };
                const rule = draft[scope].mode === "inherit" ? draft.global : draft[scope];
                if (rule.mode === "custom" && rule.url.includes("unreachable")) throw new Error("无法连接到代理");
                const latency = { content: 38, api: 41, pi: 44 }[scope];
                return { ok: true, latency_ms: latency, status: 200, message: `GitHub 可访问 · HTTP 200 · ${latency} ms` };
              }
              case "pi_generated_config":
                return structuredClone(generated);
              case "pi_available_models":
                await wait(80);
                return ["anthropic/claude-sonnet-5", "ollama/qwen2.5:14b"];
              case "pi_install_status":
                return { ...state.install };
              case "install_pi": {
                if (state.install.state === "downloading" || state.install.state === "installing") return { ...state.install };
                state.install = { ...state.install, state: "downloading", received: 0, total: 48 * 1024 * 1024, message: "" };
                if (!harness.holdInstall) setTimeout(() => { harness.progressInstall(24 * 1024 * 1024); setTimeout(harness.finishInstall, 40); }, 40);
                return { ...state.install };
              }
              case "probe_mcp":
                await wait(80);
                return structuredClone(harness.mcpProbe[args.input.name] ?? harness.mcpProbe.default);
              case "login_mcp":
                return new Promise((resolve) => { harness.mcpLogins[args.id] = resolve; });
              case "cancel_mcp_login":
                harness.mcpLogins[args.id]?.({ ok: false, message: "已取消" });
                delete harness.mcpLogins[args.id];
                return;
              case "pi_skill":
                return { name: args.name, description: "整理阅读笔记", body: "先列要点，再写结论。" };
              case "diagnostics":
                // 真实 IPC 会序列化；返回副本，免得之后原地修改改到页面缓存里的旧快照。
                return structuredClone(diag);
              case "log_set_level":
                if (!diag.log) throw new Error("日志没有初始化");
                diag.log = { ...diag.log, detailed: args.detailed };
                return structuredClone(diag.log);
              case "log_export":
                await wait(60);
                return "/home/user/Documents/rrss-diagnostics-20261105-1432.zip";
              case "log_client":
                return null;
              case "storage_page":
                await wait(80);
                return structuredClone({ ...state.storage, scanned_at: Date.now() });
              case "storage_clean": {
                await wait(120);
                // 清理后该项归零、不再提供操作；压缩数据库只清空闲空间。
                for (const group of state.storage.groups) {
                  for (const item of group.items) {
                    if (item.action !== args.action) continue;
                    Object.assign(item, { bytes: 0, size: "0", action: "", action_label: "", passive: item.free ? "无" : "", confirm: [] });
                    if (args.action === "recommendation_logs") item.hint = "0 条 · 曝光、行为与阅读会话合计 · 保留 180 天";
                  }
                }
                return args.action === "vacuum" ? "数据库已压缩。" : "已清理。";
              }
              case "retention_page":
                await wait(60);
                return structuredClone(state.retention);
              case "retention_set_days":
                state.retention.days = args.days;
                Object.assign(state.retention, args.days > 0
                  ? { preview: "将清理 12,480 篇", preview_hint: "预计释放约 420 MB，清理后可压缩数据库还给磁盘。每天刷新后自动执行；第一次执行前会自动备份。", can_run: true,
                    confirm: [`清理 12,480 篇入库满 ${args.days} 天的文章的正文、向量、译文和配图。`, "标题、链接、阅读记录和赞踩保留，仍能按标题搜到；打开时可以重新抓取正文。", "这是第一次清理，会先自动备份全部数据。"] }
                  : { preview: "", preview_hint: "", can_run: false, confirm: [] });
                return;
              case "retention_clean_now":
                await wait(120);
                Object.assign(state.retention, { preview: "目前没有要清理的文章", preview_hint: "每天刷新后自动检查一次。", can_run: false, confirm_ok: "清理" });
                return "已先自动备份。已清理 12,480 篇旧文章的正文。可以压缩数据库把空间还给磁盘。";
              case "dismiss_feed_health":
              case "unsubscribe_feeds": {
                const drop = new Set<number>(command === "unsubscribe_feeds" ? args.ids : []);
                const fix = (list: any[]) => list.filter((f) => !drop.has(f.id)).map((f) =>
                  (command === "dismiss_feed_health" && f.id === args.id ? { ...f, health: "", health_label: "", health_detail: "" } : f));
                state.feeds = { ...state.feeds, uncategorized: fix(state.feeds.uncategorized),
                  folders: state.feeds.folders.map((folder: any) => ({ ...folder, feeds: fix(folder.feeds) })) };
                return structuredClone(state.feeds);
              }
              case "backup_page":
                return structuredClone(state.backup);
              case "backup_create":
                await wait(120);
                state.backup.last_manual = "上次备份：今天 14:32。";
                return { path: "/home/user/Documents/rrss-backup-2026-09-27-1432.zip", size: "1.19 GB", summary: "文章 38,210 篇 · 订阅 74 个 · 规则 64 条 · 对话 86 个 · " + (args.includeSecrets ? "包含密钥" : "不含密钥，恢复后需重新填写") };
              case "backup_inspect":
                await wait(80);
                return {
                  path: args.path ?? "/home/user/Documents/rrss-backup-2026-09-20-2240.zip",
                  file_name: String(args.path ?? "rrss-backup-2026-09-20-2240.zip").split("/").pop(),
                  taken: "备份于 9 月 20 日 22:40 · 版本 v14",
                  summary: "文章 37,506 篇 · 订阅 72 个 · 规则 61 条 · 对话 83 个 · 不含密钥，恢复后需重新填写 API Key 与外部工具变量",
                };
              case "backup_restore":
                // 真宿主会重启；这里停住，页面保持「正在重启」。
                return new Promise(() => {});
              case "backup_cancel_restore":
              case "backup_reveal":
                return;
              case "backup_set_schedule":
                state.backup.schedule = args.value;
                state.backup.auto_summary = "升级前自动 · 定时" + ({ off: "关闭", daily: "每天", weekly: "每周" } as Record<string, string>)[args.value];
                return;
              case "backup_dismiss_notice":
                state.backup.restore_notice = null;
                return;
              case "prefs_export":
                return JSON.stringify({ format: 1, kind: "rrss-prefs", created_at: 0, rules: [] });
              case "prefs_import":
                return {
                  summary: "规则 新增 58 条，跳过重复 4 条 · 兴趣 新增 12 个 · 记忆 新增 35 条，跳过重复 2 条",
                  skipped: ["规则 不喜欢标题「广告」：限定的 https://news.example.com/rss 在本机不存在", "规则 喜欢作者「Alice」：限定的 文件夹「技术 / 博客」 在本机不存在"],
                };
              case "save_translate_settings": {
                const { deepl_key, deeplx_token, http_key, ...values } = args.args;
                Object.assign(state.translate, values);
                for (const key of ["deepl_key", "deeplx_token", "http_key"] as const) {
                  const value = args.args[key];
                  if (typeof value === "string") state.translate[key] = { state: value ? "set" : "none", hint: keyHint(value) };
                }
                if (state.translate.selection_config) state.translate.selection_config.identity += ":saved";
                return;
              }
              case "translate_page":
                return structuredClone(state.translate);
              case "translate_selection": {
                if (selectionTranslation == null) throw new Error(`mock host: unexpected command ${command}`);
                const config = structuredClone(state.translate.selection_config);
                if (!config) throw { kind: "configuration", message: "翻译配置不可用，请检查模型和服务设置" };
                if (config.identity !== args.configIdentity) throw { kind: "invalidated", message: "正文或翻译配置已更新，请重新翻译" };
                return new Promise<SelectionTranslation>((resolve, reject) => {
                  const finish = () => typeof selectionTranslation === "string"
                    ? resolve({ text: selectionTranslation, config }) : reject(selectionTranslation);
                  if (holdSelectionTranslation) pendingSelectionTranslations.push(finish); else finish();
                });
              }
              case "star_article":
                state.focus = { ...state.focus, articles: state.focus.articles.map((a) => (a.id === args.id ? { ...a, starred: args.starred } : a)) };
                return;
              case "rate_article":
                return;
              case "training_features":
                if (trainingFeatures) return trainingFeatures;
                return [];
              case "mark_read":
                return;
              case "cite_article": {
                collectWebArticle(args.id);
                const view = readerFor(args.id);
                const row = state.focus.articles.find((a) => a.id === args.id);
                return { kind: "article", article_id: args.id, title: row?.title ?? view.title, feed_title: row?.feed_title ?? "", url: row?.url ?? view.url, excerpt: "", block_id: 0 };
              }
              case "cite_selection":
                return { kind: "span", article_id: args.articleId, title: reader.title, feed_title: "Compiler Notes", url: reader.url, excerpt: args.excerpt, block_id: args.blockId };
              case "cite_digest_selection": {
                const digest = digests.find((row) => row.id === args.digestId);
                if (!digest || args.revision !== digest.revision) throw new Error("简报已更新，请重新选择");
                const selected = args.selections.flatMap((pick: { block_id: number; excerpt: string }) => {
                  const line = digest.items.find((item) => item.block_id === pick.block_id);
                  if (!line?.sources.length) throw new Error("所选简报条目没有可用来源");
                  return line.sources.map((source) => ({
                    kind: "digest", article_id: source.article_id, title: source.title,
                    feed_title: source.feed_title, url: `https://example.test/${source.article_id}`,
                    block_id: -1, excerpt: pick.excerpt,
                  }));
                });
                return selected;
              }
              case "fetch_models":
                await wait(60);
                return ["qwen2.5:14b", "qwen2.5:7b", "llama3.2", "deepseek-r1:14b", "nomic-embed-text", "mxbai-embed-large", "gemma3:12b"];
              case "interest_hits":
                return [{ id: 1, title: "sqlite-vec 在桌面端单库里的取舍", feed_title: "Compiler Notes", weight: 0.82 }];
              case "summarize_article": {
                if (summary == null && !holdSummary) throw new Error(`mock host: unexpected command ${command}`);
                const text = summary ?? "";
                if (!holdSummary) return text;
                return new Promise<string>((resolve) => { pendingSummaries.push(resolve); });
              }
              case "set_rule_action":
                state.rules.rules = state.rules.rules.map(rule => rule.id === args.id ? {
                  ...rule, enabled: args.action !== 2,
                  polarity: args.action === 2 ? rule.polarity : args.action === 0 ? "like" : "dislike",
                } : rule);
                return;
              case "update_rule":
                state.rules.rules = state.rules.rules.map(rule => rule.id === args.id ? { ...rule, ...args.spec, dimension_id: args.spec.dimension } : rule);
                return;
              case "discard_rule":
                state.rules.rules = state.rules.rules.filter(rule => rule.id !== args.id);
                return;
              case "allow_rule_for_feed": {
                const original = state.rules.rules.find(rule => rule.id === args.ruleId);
                if (!original) throw new Error("rule not found");
                const id = Math.max(98, ...state.rules.rules.map(rule => rule.id)) + 1;
                state.rules.rules.push({ ...original, id, polarity: "like", scope_kind: "feed", scope_id: args.feedId, weight: Math.abs(original.weight), enabled: true });
                return id;
              }
              case "rename_folder": {
                const folder = state.feeds.folders.find((item: { id: number }) => item.id === args.id);
                const name = String(args.name ?? "").trim();
                if (folder && name) folder.name = name;
                return null;
              }
              case "save_feed": {
                const body = args.args ?? {};
                const lists = [state.feeds.uncategorized, ...state.feeds.folders.map((folder: { feeds: { id: number; title: string }[] }) => folder.feeds)];
                for (const list of lists) {
                  const feed = list.find((item) => item.id === body.id);
                  if (feed && typeof body.title === "string") feed.title = body.title;
                }
                return state.feeds;
              }
              case "move_tree_items": {
                const moved = applyFeedTreeOrder(state.feeds, "move", { items: args.items, parentFolderId: args.parentFolderId ?? null, before: args.before ?? null });
                state.feeds = moved.tree;
                return moved.layout;
              }
              case "restore_tree_layout": {
                state.feeds = applyFeedTreeOrder(state.feeds, "restore", { layout: args.layout }).tree;
                return null;
              }
              case "create_folder": {
                const name = String(args.name ?? "").trim();
                if (!name) throw new Error("文件夹名称不能为空");
                const parent = args.parentFolderId == null ? null : Number(args.parentFolderId);
                const id = Math.max(0, ...state.feeds.folders.map((folder: { id: number }) => folder.id)) + 1;
                const parents = new Map<number, number | null>();
                const stack: { id: number; indent: number }[] = [];
                for (const folder of state.feeds.folders) {
                  while (stack.length && stack[stack.length - 1].indent >= folder.indent) stack.pop();
                  parents.set(folder.id, stack.length ? stack[stack.length - 1].id : null);
                  stack.push({ id: folder.id, indent: folder.indent });
                }
                if (parent == null) {
                  for (const folder of state.feeds.folders) if (parents.get(folder.id) == null) folder.position += 1;
                  for (const feed of state.feeds.uncategorized) feed.position += 1;
                  state.feeds.folders.unshift({ id, name, label: name, indent: 0, position: 0, feeds: [] });
                } else {
                  const index = state.feeds.folders.findIndex((folder: { id: number }) => folder.id === parent);
                  if (index < 0) throw new Error("找不到文件夹");
                  const host = state.feeds.folders[index];
                  for (const feed of host.feeds) feed.position += 1;
                  for (const folder of state.feeds.folders) if (parents.get(folder.id) === parent) folder.position += 1;
                  state.feeds.folders.splice(index + 1, 0, { id, name, label: `${host.label} / ${name}`, indent: host.indent + 1, position: 0, feeds: [] });
                }
                return id;
              }
              case "create_rule":
              case "add_interest":
                return 99;
              case "rescore":
                return 128;
              case "add_feed": {
                if (!listeners.get("feed:updated")?.size) throw new Error("feed listener must precede submission");
                const listed = [...state.feeds.uncategorized, ...state.feeds.folders.flatMap(folder => folder.feeds)];
                if (listed.some(feed => feed.url === args.url)) throw new Error("已订阅该地址");
                const id = Math.max(0, ...listed.map(feed => feed.id)) + 1;
                state.feeds.uncategorized.push({ id, url: args.url, title: args.url, folder_id: 0, position: id,
                  retention_days: null, icon_href: "", error: "", noisy: false, health: "", health_label: "", health_detail: "", empty: true });
                return { id, message: "已添加，正在后台抓取" };
              }
              case "export_opml":
                return "<opml/>";
              case "export_memories":
                return "[]";
              // 显式的记录参数替身：harness.args 在分发前保存参数；不伪造未登记命令成功。
              case "set_interest_enabled":
              case "memory_as_interest":
              case "confirm_feature":
              case "import_opml":
              case "import_mcp":
              case "delete_interest":
              case "release_article":
              case "unrelease_article":
              case "confirm_suggested_rule":
              case "apply_preset":
              case "remove_preset":
              case "confirm_memory":
              case "discard_memory":
              case "pin_memory":
              case "clear_memories":
              case "edit_memory":
              case "delete_folder":
              case "unsubscribe":
              case "toggle_pi_tool":
              case "toggle_skill":
              case "set_pi_tool_description":
              case "save_skill":
              case "delete_skill":
              case "reset_failed_embeds":
              case "embed_now":
              case "new_thread":
              case "abort_generation":
              case "stop_pi":
              case "confirm_fetch":
              case "unmark_read":
                return null;
              case "record_exposures":
                harness.signals.exposures.push(...args.records);
                return args.records.map((_: unknown, i: number) => harness.signals.exposures.length - args.records.length + i + 1);
              case "record_article_events":
                harness.signals.events.push(...args.records);
                return args.records.length;
              case "reading_speeds":
                return { chinese_original: null, chinese_translated: null, english_original: null, english_translated: null };
              case "record_read_sessions":
                harness.signals.sessions.push(...args.records);
                return args.records.length;
              default:
                throw new Error(`mock host: unexpected command ${command}`);
            }
          },
        },
      });
    },
    {
      siteRules: options.siteRules ?? { users: [], builtin_count: 1780, source: "fivefilters/ftr-site-config", commit: "045f380" },
      siteRuleReport: options.siteRuleReport ?? null,
      webExtractions: options.webExtractions ?? {},
      webWindowError: options.webWindowError ?? "", clearWebDataError: options.clearWebDataError ?? "",
      webLinks: options.webLinks ?? {},
      linkSafety: options.linkSafety ?? {},
      related: options.related ?? {},
      focus: options.focus ?? MOCK_FOCUS,
      feeds: options.feeds ?? MOCK_FEEDS,
      threads: options.threads ?? MOCK_THREADS,
      threadFolders: options.threadFolders ?? [],
      messages: options.messages ?? MOCK_MESSAGES,
      digests: options.digests ?? MOCK_DIGESTS,
      sendFailures: options.sendFailures ?? 0,
      holdSend: options.holdSend ?? false,
      ai: options.ai ? { ...MOCK_AI, ...options.ai } : MOCK_AI,
      bindings: MOCK_BINDINGS,
      pool: options.pool ?? MOCK_POOL,
      presets: MOCK_PRESETS,
      rules: options.rules ?? MOCK_RULES,
      memory: MOCK_MEMORY,
      pi: MOCK_PI,
      probe: MOCK_PI_PROBE,
      piConfig: piConfigFixture(),
      networkProxy: options.networkProxy ?? networkProxyFixture(),
      generated: generatedConfigFixture({ ignored: ["settings.cacheWarming"] }),
      settings: options.settingValues ? createSettingsSchema({ ai_daily_budget_tokens: "1000", ...options.settingValues }) : MOCK_SETTINGS,
      diag: MOCK_DIAG,
      storage: MOCK_STORAGE,
      retention: MOCK_RETENTION,
      backup: options.backup ?? MOCK_BACKUP,
      translate: options.translate ?? MOCK_TRANSLATE,
      reader: options.reader ?? MOCK_READER,
      readers: options.readers ?? {},
      readerSequences: options.readerSequences ?? {},
      holdOpen: options.holdOpen ?? false,
      holdFocusPage: options.holdFocusPage ?? false,
      holdDigest: options.holdDigest ?? false,
      focusPageSize: options.focusPageSize ?? 0,
      clusterArticles: options.clusterArticles ?? {},
      clusterSummaries: options.clusterSummaries ?? {},
      clusterGenerated: options.clusterGenerated ?? {},
      holdClusterSummary: options.holdClusterSummary ?? false,
      clusterSummaryModelReady: options.clusterSummaryModelReady ?? true,
      holdSummary: options.holdSummary ?? false,
      summary: options.summary ?? null,
      selectionTranslation: options.selectionTranslation ?? null,
      holdSelectionTranslation: options.holdSelectionTranslation ?? false,
      refreshFails: options.refreshFails ?? false,
      piDelayMs: options.piDelayMs ?? 0,
      feedIconHrefs: options.feedIconHrefs ?? {},
      brokenBlocks: options.brokenBlocks ?? {},
      trainingFeatures: options.trainingFeatures,
      vectorSpace: options.vectorSpace,
      hidden: options.hidden,
      ruleStats: options.ruleStats,
      timelineArticles: options.timelineArticles,
      feedsPageSize: options.feedsPageSize ?? 50,
      feedsHiddenCount: options.feedsHiddenCount ?? 0,
      feedTreeOrder: applyFeedTreeOrder.toString(),
      windowControls: options.windowControls ?? "windows",
    },
  );
}

/** 设置导航按钮，按分类 id 定位（见 `SettingsTabId`）；导航文字可以随时改。 */
export function settingsTab(scope: Page | Locator, id: string) {
  return scope.locator(`[data-settings-tab="${id}"]`);
}
