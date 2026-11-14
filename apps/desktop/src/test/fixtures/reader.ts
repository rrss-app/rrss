import type { ArticleView, FocusPage, ReaderView } from "../../lib/api";
import { LIBRARY_FIXTURE, LIBRARY_FIXTURE_EN, parseDocument } from "../../components/BlockView";
import { SELECTION_TRANSLATE_MAX_CHARS } from "../../bindings";

export const ARTICLE_A = 1;
export const ARTICLE_B = 2;

export function focusRow(id: number, title: string): ArticleView {
  return {
    id,
    title,
    feed_id: 1,
    feed_title: "示例信源",
    feed_icon_href: "",
    published_at: null,
    url: `https://example.test/${id}`,
    read: false,
    starred: false,
    score: 1,
    summary: "",
    row_kind: 0,
    cluster_id: 0,
    cluster_size: 1,
    cluster_label: "",
    cluster_span: "",
    interest_reason: null,
  rank_reasons: [],
    noisy: false,
    spot_check: false,
  };
}

/** 两篇文章的焦点页：甲文 = 1，乙文 = 2。 */
export const TWO_ARTICLE_PAGE: FocusPage = {
  articles: [focusRow(ARTICLE_A, "甲文"), focusRow(ARTICLE_B, "乙文")],
  digest_banner: "",
  digest_lede: "",
  filter: "all",
  status: "",
  ranking: {
    interest_boost: false,
    rerank: false,
    cluster_group: false,
    hide_noisy: false,
    interest_ready: true,
    ranking_note: "",
    hide_threshold: 0,
    theme: "system",
    rank_features: true,
    rank_source: true,
    rank_coverage: true,
    rank_freshness: true,
    rank_affinity: true,
    show_details: false,
  },
  has_more: false,
  list_scope: "all",
};

export function articleTitle(id: number) {
  return id === ARTICLE_A ? "甲文" : "乙文";
}

export function readerView(id: number, extra: Partial<ReaderView> = {}): ReaderView {
  return {
    id,
    source_hash: `source-${id}`,
    title: articleTitle(id),
    meta: "示例信源 · 刚刚",
    feed_icon_href: "",
    url: `https://example.test/${id}`,
    summary: "",
    score: 1,
    feedback: 0,
    document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: `正文 ${id}` }] }] },
    reasons: [],
    media: [],
    translated_title: "",
    translated_document: null,
    translation_ready: false,
    translation_note: "",
    truncated: false, pruned: false, summary_stale: false,
    starred: false,
    embed_covers: [],
    ...extra,
  };
}


export function translationReply(reader: ReaderView, snapshot: import("../../bindings").TranslationSnapshot | null = null): import("../../bindings").TranslationReply {
  return { kind: snapshot ? "resumed" : "missing", reader, snapshot, note: "", reconcile_after_ms: 45_000 };
}

export function libraryReaderFixture() {
  return readerView(1, { title: "开馆日", document: LIBRARY_FIXTURE, translated_title: "Opening Day", translated_document: LIBRARY_FIXTURE_EN, translation_ready: true });
}

export function selectionReaderFixture(overlong = false) {
  const view = progressiveReaderFixture().reader;
  if (!overlong) return view;
  return { ...view, document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "字".repeat(SELECTION_TRANSLATE_MAX_CHARS + 1) }] }] } };
}
export const SELECTION_TRANSLATED_TEXT = "本地优先的软件会在设备上保留可用的副本。即使在火车上，你也可以继续阅读，并在连接恢复后回到刚才的段落。";

/** 右键 / 回顶路径：真实块类型与足够跨过一屏的正文，仍复用既有语料。 */
export function readerMenuFixtures() {
  const long = progressiveReaderFixture();
  const inline = { t: "p" as const, inlines: [{ t: "t" as const, text: "检索用 " }, { t: "c" as const, text: "FTS5" }, { t: "t" as const, text: "。" }] };
  const inlineEn = { t: "p" as const, inlines: [{ t: "t" as const, text: "Search uses " }, { t: "c" as const, text: "FTS5" }, { t: "t" as const, text: "." }] };
  const document = { v: 1, blocks: [...LIBRARY_FIXTURE.blocks, inline, ...parseDocument(long.reader.document).blocks] };
  const translated = { v: 1, blocks: [...LIBRARY_FIXTURE_EN.blocks, inlineEn, ...long.complete.blocks.map(block => block.ir)] };
  return {
    plain: readerView(1, { title: "开馆日", document }),
    translated: readerView(2, { title: "开馆日", document, translated_title: "Opening Day", translated_document: translated, translation_ready: true, summary: "图书馆开放时间与借阅服务。" }),
  };
}

/** 阅读页外观对照：沿用清洗后块语料，补真实支持的语言标签与分隔块。 */
export function readerRefineFixture() {
  const view = readerMenuFixtures().translated;
  const decorate = (raw: unknown) => ({ v: 1, blocks: [
    ...parseDocument(raw).blocks.map(block => block.t === "pre" ? { ...block, lang: "text" } : block),
    { t: "hr" as const },
  ] });
  return { ...view, id: 1, title: "Opening Day", translated_title: "开馆日",
    media: [{ src: "cover.png", href: "/reader-refine-cover.png", width: 480, height: 240, error: "" }],
    document: decorate(view.translated_document), translated_document: decorate(view.document) };
}

export function translationSnapshot(reader: ReaderView = readerView(ARTICLE_A), extra: Partial<import("../../bindings").TranslationSnapshot> = {}): import("../../bindings").TranslationSnapshot {
  return {
    identity: { article_id: reader.id, generation: 10, source_hash: reader.source_hash, config_hash: "config-1" },
    seq: 1, phase: "queued", save: "unsaved", units: ["pending", "pending"], unit_map: [null, 0], title: null, blocks: [], error: null,
    ...extra,
  };
}

/** 长文渐进场景，组件与浏览器共享相同的原文 / 译文及预设终态。 */
export function progressiveReaderFixture() {
  const originals = Array.from({ length: 18 }, (_, i) => ({ t: "p" as const, inlines: [{ t: "t" as const,
    text: `Part ${i + 1}. Local-first software keeps a useful copy on your device. You can continue reading on a train, preserve your work during an interruption, and return to the same paragraph when a connection becomes available.` }] }));
  const translated = originals.map((_, i) => ({ t: "p" as const, inlines: [{ t: "t" as const,
    text: `第 ${i + 1} 段。本地优先的软件在设备上保留一份可用的副本。你可以在火车上继续阅读，在中断时保留工作，并在连接恢复后回到刚才的段落。` }] }));
  const reader = readerView(1, { source_hash: "progressive-source", title: "Local-first reading", document: { v: 1, blocks: originals } });
  const unit_map = [null, ...originals.map((_, i) => i)];
  const queued = translationSnapshot(reader, { units: unit_map.map(() => "pending"), unit_map });
  const make = (states: import("../../bindings").TranslationUnitState[], phase: import("../../bindings").TranslationPhase, save: import("../../bindings").TranslationSaveState) => ({
    ...queued, phase, save, units: states, title: states[0] === "done" ? "本地优先的阅读" : null,
    blocks: translated.flatMap((ir, index) => states[index + 1] === "done" ? [{ index, ir }] : []),
  });
  return { reader, queued,
    first: make(unit_map.map((_, i) => i < 3 ? "done" : "pending"), "running", "unsaved"),
    partial: make(unit_map.map((_, i) => i === 4 ? { failed: "network" } : "done"), "partial", "saved"),
    retrying: make(unit_map.map((_, i) => i === 4 ? "inflight" : "done"), "running", "unsaved"),
    complete: make(unit_map.map(() => "done"), "completed", "saved"),
    saveFailed: { ...make(unit_map.map(() => "done"), "completed", "save_failed"), error: "storage" as const },
    allFailed: { ...make(unit_map.map(() => ({ failed: "network" as const })), "failed", "unsaved"), error: "network" as const },
  };
}

/** 返回定位：行高不同的长列表，以及可按块恢复的正文。 */
export function readerReturnFixture(count = 24) {
  const document = progressiveReaderFixture().reader.document;
  const articles = Array.from({ length: count }, (_, i) => ({
    ...focusRow(i + 1, `阅读位置 ${i + 1}${i % 3 === 0 ? ' · 本地优先阅读与长列表中的块锚点恢复'.repeat(3) : ''}`),
    summary: i % 2 === 0 ? "换篇、返回列表后仍可以继续阅读。".repeat(8) : "",
  }));
  return {
    focus: { ...TWO_ARTICLE_PAGE, articles },
    readers: Object.fromEntries(articles.map(row => [row.id, readerView(row.id, { title: row.title, document })])),
  };
}
