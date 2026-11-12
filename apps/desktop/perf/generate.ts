import { threadFixture } from "../src/test/fixtures/chat";
import { aiPageFixture, providerFixture } from "../src/test/fixtures/settings";
import type {
  AiPage,
  ArticleView,
  ChatMessageView,
  DiagnosticsPage,
  DigestView,
  FeedTree,
  FocusPage,
  MediaRef,
  MemoryPage,
  PiPage,
  RankingSettings,
  ReaderView,
  RulesPage,
  ThreadView,
  TranslatePage,
} from "../src/bindings";
import type { Block, Document } from "../src/components/BlockView";
import {
  MOCK_DIAG,
  MOCK_DIGESTS,
  MOCK_FEEDS,
  MOCK_MEMORY,
  MOCK_PI,
  MOCK_RULES,
  MOCK_TRANSLATE,
} from "../e2e/mockHost";
import { isLoopbackMediaHref, looksLikeRemoteImageUrl, mediaHref, mediaToken } from "./media";
import {
  DIGEST_THREAD_ID,
  LONG_ARTICLE_ID,
  LONG_ARTICLE_TITLE,
  LONG_THREAD_ID,
  LONG_THREAD_TITLE,
  PERF_ORIGIN,
  PERF_TIME_ORIGIN_MS,
  SCENES,
  type SceneName,
  type SceneSpec,
} from "./scenes";

export type GeneratedScene = {
  spec: SceneSpec;
  origin: string;
  articles: ArticleView[];
  longArticleId: number;
  longReader: ReaderView;
  shortDocument: Document;
  threads: ThreadView[];
  longThreadId: number;
  messages: Record<number, ChatMessageView[]>;
  feeds: FeedTree;
  digests: DigestView[];
  digestThreadId: number;
  digestMessages: ChatMessageView[];
  ranking: RankingSettings;
  filter: string;
  listScope: string;
  digestBanner: string;
  ai: AiPage;
  rules: RulesPage;
  memory: MemoryPage;
  pi: PiPage;
  diag: DiagnosticsPage;
  translate: TranslatePage;
};

type Rng = () => number;

function mulberry32(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function pick<T>(rng: Rng, items: readonly T[]): T {
  const item = items[Math.floor(rng() * items.length)];
  if (item === undefined) throw new Error("empty pick");
  return item;
}

function pad(n: number, width: number): string {
  return String(n).padStart(width, "0");
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function feedIds(tree: FeedTree): number[] {
  const ids = [
    ...tree.uncategorized.map((feed) => feed.id),
    ...tree.folders.flatMap((folder) => folder.feeds.map((feed) => feed.id)),
  ];
  if (ids.some((id) => id <= 0) || new Set(ids).size !== ids.length) {
    throw new Error("feed fixture ids must be unique positive integers");
  }
  return ids;
}

function feedTitle(tree: FeedTree, id: number): string {
  for (const feed of tree.uncategorized) {
    if (feed.id === id) return feed.title;
  }
  for (const folder of tree.folders) {
    for (const feed of folder.feeds) {
      if (feed.id === id) return feed.title;
    }
  }
  return `feed-${id}`;
}

function publishedAt(id: number): number {
  return PERF_TIME_ORIGIN_MS + (id - 1) * 3_600_000;
}

function publishedLabel(ms: number | null): string {
  const at = new Date(ms ?? PERF_TIME_ORIGIN_MS);
  const mm = pad(at.getUTCMonth() + 1, 2);
  const dd = pad(at.getUTCDate(), 2);
  const hh = pad(at.getUTCHours(), 2);
  const mi = pad(at.getUTCMinutes(), 2);
  return `固定时刻 ${mm}-${dd} ${hh}:${mi}`;
}

function textRun(id: number, kind: string): string {
  return `合成${kind}${pad(id, 4)} 固定种子内容，不含密钥。`;
}

function paragraph(id: number): Block {
  return { t: "p", inlines: [{ t: "t", text: textRun(id, "段落") }] };
}

function heading(id: number): Block {
  return { t: "h", level: 2, inlines: [{ t: "t", text: textRun(id, "标题") }] };
}

function listItem(id: number): Block {
  return { t: "li", indent: 0, marker: "•", inlines: [{ t: "t", text: textRun(id, "列表") }] };
}

function quote(id: number): Block {
  return { t: "q", inlines: [{ t: "t", text: textRun(id, "引用") }] };
}

function imageBlock(mediaId: number): Block {
  return { t: "img", src: mediaToken(mediaId), alt: `合成配图 ${mediaId}` };
}

function buildBody(
  blockCount: number,
  imageCount: number,
  rng: Rng,
  origin: string,
): { document: Document; media: MediaRef[] } {
  if (imageCount > blockCount) throw new Error("imageCount cannot exceed bodyBlockCount");
  const kinds: Array<"p" | "h" | "li" | "q"> = ["p", "h", "li", "q"];
  const blocks: Block[] = [];
  const media: MediaRef[] = [];
  const imageSlots = new Set<number>();
  while (imageSlots.size < imageCount) {
    imageSlots.add(Math.floor(rng() * blockCount));
  }
  let mediaId = 0;
  for (let i = 0; i < blockCount; i++) {
    if (imageSlots.has(i)) {
      mediaId += 1;
      blocks.push(imageBlock(mediaId));
      media.push({
        src: mediaToken(mediaId),
        media_id: mediaId,
        href: mediaHref(mediaId, origin),
        width: null,
        height: null,
        error: "",
      });
      continue;
    }
    const kind = pick(rng, kinds);
    const id = i + 1;
    if (kind === "h") blocks.push(heading(id));
    else if (kind === "li") blocks.push(listItem(id));
    else if (kind === "q") blocks.push(quote(id));
    else blocks.push(paragraph(id));
  }
  return { document: { v: 1, blocks }, media };
}

function articleView(
  id: number,
  feedId: number,
  title: string,
  feed: string,
  extra: Partial<ArticleView> = {},
): ArticleView {
  return {
    id,
    title,
    feed_id: feedId,
    feed_title: feed,
    feed_icon_href: "",
    published_at: publishedAt(id),
    url: `https://example.test/perf/${id}`,
    read: extra.read ?? false,
    starred: extra.starred ?? false,
    score: extra.score ?? 1,
    summary: extra.summary ?? textRun(id, "摘要"),
    row_kind: 0,
    cluster_id: 0,
    cluster_size: 1,
    cluster_label: "",
    cluster_span: "",
    interest_label: extra.interest_label ?? "",
    noisy: extra.noisy ?? false,
    spot_check: extra.spot_check ?? false,
  };
}

function messageView(id: number, role: "user" | "assistant", text: string, document: Document | null = null): ChatMessageView {
  return {
    id,
    role,
    text,
    streaming: false,
    cite_article_id: 0,
    cite_title: "",
    citations: [],
    document,
  };
}

function buildHistory(count: number): ChatMessageView[] {
  const rows: ChatMessageView[] = [];
  for (let i = 1; i <= count; i++) {
    const role = i % 2 === 1 ? "user" : "assistant";
    const text = textRun(i, role === "user" ? "提问" : "回复");
    const document =
      role === "assistant" && i % 10 === 0
        ? { v: 1, blocks: [paragraph(i), heading(i)] }
        : null;
    rows.push(messageView(i, role, text, document));
  }
  return rows;
}

// 前端只读连接列表与权限；其余字段取共享默认值。
const PERF_AI: AiPage = aiPageFixture({
  providers: [providerFixture({ name: "合成接口", base_url: "http://127.0.0.1:9/v1" })],
  selected_id: 1,
});

export function generateScene(name: SceneName, origin = PERF_ORIGIN): GeneratedScene {
  const spec = SCENES[name];
  const rng = mulberry32(spec.seed);
  const feeds = cloneJson(MOCK_FEEDS);
  const ids = feedIds(feeds);
  const articles: ArticleView[] = [];
  for (let i = 1; i <= spec.articleCount; i++) {
    const feedId = ids[(i - 1) % ids.length] ?? ids[0]!;
    const title = i === LONG_ARTICLE_ID ? LONG_ARTICLE_TITLE : `合成文章 ${pad(i, 4)}`;
    articles.push(
      articleView(i, feedId, title, feedTitle(feeds, feedId), {
        starred: i % 17 === 0,
        read: i % 11 === 0,
        noisy: feedId === 1,
      }),
    );
  }
  const body = buildBody(spec.bodyBlockCount, spec.imageCount, rng, origin);
  const longRow = articles[0];
  if (!longRow) throw new Error("scene has no articles");
  const longReader: ReaderView = {
    source_hash: `perf-${LONG_ARTICLE_ID}`,
    id: LONG_ARTICLE_ID,
    title: longRow.title,
    meta: `${longRow.feed_title} · ${publishedLabel(longRow.published_at)}`, feed_icon_href: "",
    url: longRow.url,
    summary: longRow.summary,
    score: longRow.score,
    feedback: 0,
    document: body.document,
    reasons: [],
    media: body.media,
    translated_title: "",
    translated_document: null,
    translation_ready: false,
    translation_note: "",
    truncated: false, pruned: false, summary_stale: false,
    starred: longRow.starred,
    embed_covers: [],
  };
  const shortDocument: Document = {
    v: 1,
    blocks: [paragraph(1), heading(2), paragraph(3)],
  };
  const history = buildHistory(spec.historyMessageCount);
  const translate = cloneJson(MOCK_TRANSLATE);
  return {
    spec,
    origin,
    articles,
    longArticleId: LONG_ARTICLE_ID,
    longReader,
    shortDocument,
    threads: [threadFixture({ id: LONG_THREAD_ID, title: LONG_THREAD_TITLE })],
    longThreadId: LONG_THREAD_ID,
    messages: {
      [LONG_THREAD_ID]: history,
    },
    feeds,
    digests: cloneJson(MOCK_DIGESTS),
    digestThreadId: DIGEST_THREAD_ID,
    digestMessages: [
      messageView(11, "user", "展开第二条"),
      messageView(12, "assistant", "第二条为合成简报追问回复。", {
        v: 1,
        blocks: [paragraph(12)],
      }),
    ],
    ranking: {
      interest_boost: false,
      rerank: false,
      cluster_group: false,
      hide_noisy: false,
      interest_ready: false,
      ranking_note: "合成场景：聚类关闭，兴趣未参与",
      hide_threshold: -10,
      rank_features: false,
      rank_source: false,
      rank_coverage: false,
      rank_freshness: false,
      rank_affinity: false,
      show_details: false,
      theme: "light",
    },
    filter: "all",
    listScope: "all",
    digestBanner: "今日简报 · 合成日",
    ai: cloneJson(PERF_AI),
    rules: cloneJson(MOCK_RULES),
    memory: cloneJson(MOCK_MEMORY),
    pi: cloneJson(MOCK_PI),
    diag: cloneJson(MOCK_DIAG),
    translate,
  };
}

export type FocusQuery = {
  feedId: number | null;
  more: boolean;
  excludeIds: number[];
  excludeClusterIds: number[];
  filter: string;
  listScope: string;
};

export function filterArticles(
  articles: readonly ArticleView[],
  query: { feedId: number | null; listScope: string },
): ArticleView[] {
  return articles.filter((row) => {
    if (row.row_kind !== 0) return false;
    if (query.feedId !== null && row.feed_id !== query.feedId) return false;
    if (query.listScope === "starred") return row.starred;
    if (query.listScope === "unread") return !row.read;
    return true;
  });
}

/** 与生产 list_focus_page 相近：exclude 后取 pageSize+1 判断 has_more。 */
export function paginateArticles(
  articles: readonly ArticleView[],
  query: FocusQuery,
  pageSize: number,
): { rows: ArticleView[]; has_more: boolean } {
  const excluded = new Set(query.excludeIds);
  const filtered = filterArticles(articles, query).filter((row) => !excluded.has(row.id));
  const want = pageSize + 1;
  const sliced = filtered.slice(0, want);
  const has_more = sliced.length > pageSize;
  return { rows: has_more ? sliced.slice(0, pageSize) : sliced, has_more };
}

export function focusPageFrom(
  scene: GeneratedScene,
  query: FocusQuery,
): FocusPage {
  const { rows, has_more } = paginateArticles(scene.articles, query, scene.spec.pageSize);
  return {
    articles: cloneJson(rows),
    digest_banner: scene.digestBanner,
    digest_lede: "",
    filter: query.filter,
    status: "",
    ranking: cloneJson(scene.ranking),
    has_more,
    list_scope: query.listScope,
  };
}

export function streamSnapshots(fullText: string, count: number): Array<{ text: string; document: Document }> {
  if (count <= 0) return [];
  const unit = Math.max(1, Math.ceil(fullText.length / count));
  const out: Array<{ text: string; document: Document }> = [];
  for (let i = 1; i <= count; i++) {
    const text = fullText.slice(0, Math.min(fullText.length, i * unit));
    out.push({
      text,
      document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text }] }] },
    });
  }
  const last = out[out.length - 1];
  if (last && last.text !== fullText) {
    last.text = fullText;
    last.document = { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: fullText }] }] };
  }
  return out;
}

export function sceneMediaRefs(scene: GeneratedScene): MediaRef[] {
  return scene.longReader.media;
}

export function assertSceneInvariants(scene: GeneratedScene): void {
  const { spec, articles, longReader, messages } = scene;
  if (articles.length !== spec.articleCount) throw new Error("article count mismatch");
  if (new Set(articles.map((row) => row.id)).size !== articles.length) throw new Error("duplicate article id");
  if (articles.some((row) => row.id <= 0)) throw new Error("invalid article id");
  const history = messages[scene.longThreadId] ?? [];
  if (history.length !== spec.historyMessageCount) throw new Error("history count mismatch");
  if (history.some((row) => row.id <= 0)) throw new Error("invalid message id");
  const doc = longReader.document as Document;
  if (doc.blocks.length !== spec.bodyBlockCount) throw new Error("block count mismatch");
  if (longReader.media.length !== spec.imageCount) throw new Error("image count mismatch");
  if (spec.samples < 20) throw new Error("samples must be >= 20");
  if (spec.streamSnapshotCount < 60) throw new Error("stream snapshots must be >= 60");
  if (spec.streamIntervalMs !== 30) throw new Error("stream interval must be 30ms");
  for (const block of doc.blocks) {
    if (block.t === "img") {
      if (looksLikeRemoteImageUrl(block.src)) throw new Error("img src is a remote url");
      if (isLoopbackMediaHref(block.src)) throw new Error("img src must be a fixture token, not href");
    }
    if (block.t === "embed") {
      throw new Error("unexpected embed block in synthetic body");
    }
  }
  for (const item of longReader.media) {
    if (!isLoopbackMediaHref(item.href)) throw new Error("media href must be loopback fixture");
    if (looksLikeRemoteImageUrl(item.src)) throw new Error("media src is a remote url");
    if (item.href === item.src) throw new Error("media src aliasing href");
    if (!item.href.includes("://")) throw new Error("media href missing scheme");
  }
}

export function shortReaderFor(scene: GeneratedScene, id: number): ReaderView {
  const row = scene.articles.find((article) => article.id === id);
  if (!row) throw new Error(`unknown article ${id}`);
  if (id === scene.longArticleId) return cloneJson(scene.longReader);
  return {
    id,
    source_hash: `perf-${id}`,
    title: row.title,
    meta: `${row.feed_title} · ${publishedLabel(row.published_at)}`, feed_icon_href: "",
    url: row.url,
    summary: row.summary,
    score: row.score,
    feedback: 0,
    document: cloneJson(scene.shortDocument),
    reasons: [],
    media: [],
    translated_title: "",
    translated_document: null,
    translation_ready: false,
    translation_note: "",
    truncated: false, pruned: false, summary_stale: false,
    starred: row.starred,
    embed_covers: [],
  };
}
