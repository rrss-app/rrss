import { useState } from "react";
import type { SearchBlockHitView, SearchHitView } from "../../lib/api";
import { SourceAvatar } from "../../components/ArticleCard";
import { RelativeTime } from "../../components/RelativeTime";
import { useTranslation } from "../../i18n";
import { focusOnMatch, Highlight } from "./Highlight";
import type { HitBlocks } from "./useHitBlocks";

/** 默认显示的命中段数，其余折叠。 */
const HITS_FOLD = 4;
/** 后端最多返回的命中段数；到顶时写「50+ 处」。 */
const HITS_MAX = 50;
const SIBLINGS_MAX = 5;

export function Score({ value }: { value: number }) {
  return <span className="search-score" data-tone={value > 0 ? "pos" : undefined}>{value > 0 ? `+${value}` : value}</span>;
}

const hitCount = (n: number) => (n >= HITS_MAX ? `${HITS_MAX}+` : `${n}`);

export type SearchPreviewProps = {
  hit: SearchHitView;
  query: string;
  blocks: HitBlocks;
  /** 阅读时长（分钟）；0 为未知，不显示。 */
  minutes?: number;
  /** 宽档才给：本次结果，用来在命中位置旁标出「标题 / 正文 N 处」并在底部列同来源（只从本次结果里取）。 */
  results: SearchHitView[] | null;
  onOpen: (id: number, at?: { blockId: number }) => void;
};

/**
 * 预览：来源、标题、时间与分数；正文命中段（位置条 + 按小标题归组的整段）。不显示 AI 摘要。
 * 宽档仍是两栏（结果 + 预览）：命中位置并进元信息行，同来源放在预览底部，键位只在底栏。
 */
export function SearchPreview({ hit, query, blocks, minutes = 0, results, onOpen }: SearchPreviewProps) {
  const { t } = useTranslation("search");
  const hits = blocks.status === "ready" ? blocks.data!.hits : null;
  const bodyOn = hits ? hits.length > 0 : hit.body_match;
  const siblings = results ? results.filter((x) => x.id !== hit.id && x.feed_title === hit.feed_title).slice(0, SIBLINGS_MAX) : [];
  return (
    <article className="search-preview-main" aria-label={t("preview.label")}>
      <div className="search-preview-source"><SourceAvatar title={hit.feed_title} size={16} iconHref={hit.feed_icon_href} />{hit.feed_title}</div>
      <h2><Highlight text={hit.title} query={query} /></h2>
      <div className="search-preview-meta">
        {hit.published_at != null ? <><RelativeTime ms={hit.published_at} /> · </> : null}
        {minutes > 0 ? <>{t("preview.minutes", { count: minutes })} · </> : null}{t("preview.score")} <Score value={hit.score} />
        {hit.starred ? <> · {t("state.starred")}</> : null}
        {results ? <span className="search-where" role="group" aria-label={t("preview.where")}>
          <span data-on={hit.title_match || undefined}>{t("preview.whereTitle")}</span>
          <span data-on={bodyOn || undefined}>{hits ? t("preview.whereBodyCount", { n: hitCount(hits.length) }) : t("preview.whereBody")}</span>
        </span> : null}
      </div>
      {hits && hits.length > 0 ? (
        <HitMap hits={hits} total={blocks.data!.total} query={query} onOpenAt={(blockId) => onOpen(hit.id, { blockId })} />
      ) : hit.snippet ? (
        // 取命中段途中先显示结果行的片段，不闪空；只标题命中或取不到时就停在这里。
        <section className="search-preview-context">
          <h4>{hit.body_match ? t("preview.bodyHit") : t("preview.bodyStart")}</h4>
          <p><Highlight text={hit.snippet} query={query} /></p>
          {blocks.status === "error" ? <small className="search-preview-note">{t("preview.blocksError")}</small> : null}
        </section>
      ) : null}
      <p className="search-preview-go">{hit.body_match ? t("preview.goBody") : t("preview.goTitle")}</p>
      {siblings.length ? (
        <section className="search-siblings" aria-label={t("preview.siblings")}>
          <h4>{t("preview.siblingsTitle")}</h4>
          <div className="search-siblings-list">{siblings.map((x) => (
            <button key={x.id} type="button" className="search-sibling" onClick={() => onOpen(x.id)}>
              <Highlight text={x.title} query={query} />
              {x.published_at != null ? <small><RelativeTime ms={x.published_at} /></small> : null}
            </button>
          ))}</div>
        </section>
      ) : null}
    </article>
  );
}

/** 正文命中 N 处：位置条标出各处在全文中的位置；按小标题归组整段显示，点哪段打开到哪段。 */
function HitMap({ hits, total, query, onOpenAt }: { hits: SearchBlockHitView[]; total: number; query: string; onOpenAt: (blockId: number) => void }) {
  const { t } = useTranslation("search");
  const [expanded, setExpanded] = useState(false);
  const [hover, setHover] = useState<number | null>(null);
  const shown = expanded ? hits : hits.slice(0, HITS_FOLD);
  const rest = hits.length - shown.length;
  const span = Math.max(total, 1);
  return (
    <section className="search-hits" aria-label={t("hits.label")}>
      <div className="search-hits-head"><h4>{t("hits.head", { n: hitCount(hits.length) })}</h4><span>{t("hits.total", { count: total })}</span></div>
      <div className="search-strip" aria-hidden="true">
        {hits.map((h) => (
          <i key={h.block_id} data-on={hover === h.block_id || undefined}
            style={{ left: `${Math.min(100, Math.max(0, ((h.ordinal - 0.5) / span) * 100)).toFixed(2)}%` }} />
        ))}
      </div>
      {shown.map((h, i) => (
        <div key={h.block_id} className="search-hit-item">
          {/* 标题本身命中时它就排在自己这组上面，不再重复组名。 */}
          {h.section && h.section !== shown[i - 1]?.section && h.section !== shown[i - 1]?.text
            ? <div className="search-hits-section"><Highlight text={h.section} query={query} /></div> : null}
          <button type="button" className="search-hit" onClick={() => onOpenAt(h.block_id)}
            onMouseEnter={() => setHover(h.block_id)} onMouseLeave={() => setHover(null)}
            onFocus={() => setHover(h.block_id)} onBlur={() => setHover(null)}>
            <span className="search-hit-text"><Highlight text={focusOnMatch(h.text, query)} query={query} /></span>
            <span className="search-hit-foot">{t("hits.ordinal", { n: h.ordinal })}<span className="search-hit-go">{t("hits.go")}</span></span>
          </button>
        </div>
      ))}
      {rest > 0 ? <button type="button" className="search-hits-more" onClick={() => setExpanded(true)}>{t("hits.more", { count: rest })}</button> : null}
    </section>
  );
}
