import type { ArticleView, DigestSourceView, DigestView, FeedTree, FocusPage, MemoryPage, RulesPage, ThreadView } from "../bindings";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const HOUR_MS = 3_600_000;
// 预览数据的发布时间相对加载时刻，列表里显示的相对说法与真实宿主一致。
const hoursAgo = (hours: number) => Date.now() - hours * HOUR_MS;

const sample = (
  id: number,
  title: string,
  feed: string,
  publishedAt: number | null,
  extra: Partial<ArticleView> = {},
): ArticleView => ({
  id,
  title,
  feed_id: 1,
  feed_title: feed,
  feed_icon_href: "",
  published_at: publishedAt,
  url: "https://example.test",
  read: extra.read ?? false,
  starred: extra.starred ?? false,
  score: extra.score ?? 0,
  summary: extra.summary ?? "",
  row_kind: extra.row_kind ?? 0,
  cluster_id: extra.cluster_id ?? 0,
  cluster_size: extra.cluster_size ?? 1,
  cluster_label: extra.cluster_label ?? "",
  cluster_span: extra.cluster_span ?? "",
  interest_reason: extra.interest_reason ?? null,
  noisy: extra.noisy ?? false,
  spot_check: extra.spot_check ?? false,
});

export const PREVIEW_FOCUS: FocusPage = {
  filter: "hide_faded",
  list_scope: "all",
  status: "今日 token 0 · 待摘要 0",
  digest_banner: "今日简报 · 9月16日",
  digest_lede: "",
  has_more: false,
  ranking: {
    interest_boost: true,
    rerank: false,
    cluster_group: true,
    hide_noisy: false,
    interest_ready: false,
    ranking_note: "兴趣未就绪：还没有可用的兴趣向量，当前按时间混合",
    hide_threshold: 0,
    theme: "system",
    rank_features: true,
    rank_source: true,
    rank_coverage: true,
    rank_freshness: true,
    rank_affinity: true,
    show_details: false,
  },
  articles: [
    sample(1, "我用旧平板做了一块桌面信息屏", "闲聊广场", hoursAgo(0.4), {
      summary: "用一组小工具把刘海区变成状态栏，专门盯本地模型和 RSS 任务。",
      score: 3,
      noisy: true,
    }),
    sample(10, "", "", hoursAgo(26), {
      row_kind: 1,
      cluster_id: 10,
      cluster_size: 2,
      cluster_label: "Rust 发行",
      cluster_span: "08-20 → 08-26",
    }),
    sample(2, "编译器 1.97.1 点发行说明", "Compiler Notes", hoursAgo(26), {
      read: true,
      summary: "修复若干稳定版回归，推荐尽快升级。",
      cluster_id: 10,
      cluster_size: 2,
      cluster_label: "Rust 发行",
      cluster_span: "08-20 → 08-26",
    }),
    sample(3, "sqlite-vec 在桌面端单库里的取舍", "English", hoursAgo(80), {
      summary: "不必上 Qdrant：WAL + FTS5 已经够用到十万级。",
      score: 2,
      interest_reason: { code: "legacy_text", params: { text: " · 本地优先", prefix: null } },
    }),
  ],
};

export const PREVIEW_FEEDS: FeedTree = {
  folders: [
    {
      id: 1,
      name: "中文",
      label: "中文",
      indent: 0,
      position: 0,
      feeds: [
        { id: 1, title: "闲聊广场", url: "https://xianliao.example/index.xml", error: null, noisy: true, folder_id: 1, icon_href: "", retention_days: null, health: null, position: 0, empty: false },
      ],
    },
    {
      id: 2,
      name: "English",
      label: "English",
      indent: 0,
      position: 1,
      feeds: [
        { id: 2, title: "Compiler Notes", url: "https://compiler-notes.example/feed.xml", error: null, noisy: false, folder_id: 2, icon_href: "", retention_days: null, health: null, position: 0, empty: false },
      ],
    },
  ],
  uncategorized: [
    { id: 3, title: "gadgetwire", url: "http://192.0.2.10:1200/gadgetwire", error: { code: "feed_refresh_http", params: { status: 503 }, detail: "http status 503" }, noisy: false, folder_id: 0, icon_href: "", retention_days: null, health: null, position: 2, empty: false },
  ],
  folder_combo: ["中文", "English"],
};

export const PREVIEW_THREADS: ThreadView[] = [{ id: 1, title: "查证编译器发行说明", chat_model: null, chat_thinking: null, context: null, folder_id: null, pinned: false, updated_at: 0, archived_at: null }];

export const PREVIEW_MEMORY: MemoryPage = {
  memories: [{ id: 1, kind: "偏好", content: "偏好本地优先工具（预览）", pinned: false, pending: true, scope: "", strength: "" }],
  interests: [{ id: 1, label: "本地优先（预览）", weight: 1, enabled: true, has_embedding: false, hit_count: 0 }],
  inject_preview: "",
};

export const PREVIEW_RULES: RulesPage = {
  rules: [{ id: 1, summary: { code: "rule_summary", params: { effect: "boost" } }, scope_name: "", polarity: "like", dimension_id: "title_kw", pattern: "Rust", scope_kind: "global", scope_id: 0, weight: 10, source: "manual", enabled: true, pending: false, pack_id: "" }],
  presets: [{ id: "dev-focus", name: "开发关注", description: "Rust 等", rule_count: 3, enabled_count: 0, installed: false }],
  folders: [], feeds: [], hide_threshold: -10,
};



const previewDigestSources: DigestSourceView[] = [
  { article_id: 1, title: "我用旧平板做了一块桌面信息屏", feed_title: "闲聊广场", number: 1, summary: null, published_at: null, status: "available" },
  { article_id: 2, title: "编译器 1.97.1 点发行说明", feed_title: "Compiler Notes", number: 2, summary: null, published_at: null, status: "available" },
];

export const PREVIEW_DIGESTS: DigestView[] = [
  {
    id: 1,
    period: "daily",
    period_label: "日报",
    period_start: 1_756_224_000_000,
    date_label: "8月27日",
    body: "### 今日三条主线\n\n**加粗**\n\n- 列表项",
    revision: "preview-daily-revision-1",
    source_state: "legacy",
    source_note: "旧稿来源仅按存储位置显示",
    source_index: previewDigestSources,
    unlocated_sources: [],
    items: [{ block_id: 2, sources: previewDigestSources }],
    document: {
      v: 1,
      blocks: [
        { t: "h", level: 3, inlines: [{ t: "t", text: "今日三条主线" }] },
        { t: "p", inlines: [{ t: "s", children: [{ t: "t", text: "加粗" }] }] },
        { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: "列表项" }] },
      ],
    },
  },
  {
    id: 2,
    period: "daily",
    period_label: "日报",
    period_start: 1_756_137_600_000,
    date_label: "8月26日",
    body: "### 昨日主线",
    revision: "preview-daily-revision-2",
    source_state: "unavailable",
    source_note: "",
    source_index: [],
    unlocated_sources: [],
    items: [],
    document: {
      v: 1,
      blocks: [{ t: "h", level: 3, inlines: [{ t: "t", text: "昨日主线" }] }],
    },
  },
  {
    id: 3,
    period: "weekly",
    period_label: "周报",
    period_start: 1_756_051_200_000,
    date_label: "8月25日 – 8月31日",
    body: "### 本周三条主线",
    revision: "preview-weekly-revision-1",
    source_state: "unavailable",
    source_note: "",
    source_index: [],
    unlocated_sources: [],
    items: [],
    document: {
      v: 1,
      blocks: [{ t: "h", level: 3, inlines: [{ t: "t", text: "本周三条主线" }] }],
    },
  },
];
