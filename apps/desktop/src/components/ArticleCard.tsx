import { useState, type CSSProperties } from "react";
import type { ArticleView } from "../lib/api";
import type { RankReasonView } from "../bindings";
import { localIconHref, sourceHue, sourceInitial } from "../lib/sourceAvatar";
import { useTranslation } from "../i18n";
import { formatReason, formatReasonNumber } from "../lib/reasons";
import { Icon } from "./Icon";
import { RelativeTime } from "./RelativeTime";
import { ThumbIcon } from "./ThumbIcon";
import { SpotCheckReveal, type SpotAction, type SpotReview } from "./SpotCheckReveal";

export function SourceAvatar({ title, size = 16, iconHref = "" }: { title: string; size?: number; iconHref?: string }) {
  const [failedHref, setFailedHref] = useState("");
  const showIcon = !!localIconHref(iconHref) && iconHref !== failedHref;
  return (
    <span
      className="source-avatar"
      aria-hidden="true"
      data-source-avatar={showIcon ? "image" : "initial"}
      style={{ "--avatar-hue": sourceHue(title), width: size, height: size, fontSize: Math.round(size * 0.58) } as CSSProperties}
    >
      {showIcon ? <img src={iconHref} alt="" width={size} height={size} onError={() => setFailedHref(iconHref)} /> : sourceInitial(title)}
    </span>
  );
}

/** 一篇文章的完整卡片。相近报道的组头、成员行与组尾见 `FocusCluster`。 */
export function ArticleCard({
  row, focused = false, representative = false, details = true, review, reviewBusy = false, hideThreshold = 0, onReview, onOpen, onCite, onRate, onStar, onSource,
}: {
  row: ArticleView;
  focused?: boolean;
  /** 相近报道组里的代表文章：收起时随组头显示，展开后固定在组首。 */
  representative?: boolean;
  details?: boolean;
  review?: SpotReview;
  reviewBusy?: boolean;
  hideThreshold?: number;
  onReview?: (action: SpotAction) => void;
  onOpen: (id: number) => void;
  onCite: (id: number) => void;
  /** 点来源名：进入订阅模式并选中这个源。 */
  onSource?: (feedId: number) => void;
  onRate: (row: ArticleView, up: boolean) => void;
  onStar?: (row: ArticleView) => void;
}) {
  const { t } = useTranslation("focus");
  const score = review?.article.score ?? row.score;
  const scoreClass = score > 0 ? "up" : "down";
  const showReasons = details && (!row.spot_check || !!review);
  // 学来的原因只露加分最多的一条，其余放在悬停提示里。
  const learned = row.rank_reasons?.reduce<RankReasonView | undefined>((best, r) => r.points > 0 && r.points > (best?.points ?? 0) ? r : best, undefined);
  return (
    <article
      className={`article${row.read ? "" : " unread"}${row.starred ? " starred" : ""}${focused ? " focused" : ""}`}
      tabIndex={0}
      aria-label={row.title}
      onClick={() => onOpen(row.id)}
      onKeyDown={(event) => {
        if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
          event.preventDefault();
          event.stopPropagation();
          onOpen(row.id);
        }
      }}
    >
      <div className="body">
        <h3>{row.title}</h3>
        {row.summary ? <p className="summary">{row.summary}</p> : null}
        <div className="article-footer">
          <div className="meta">
            <SourceAvatar title={row.feed_title} iconHref={row.feed_icon_href} />
            {onSource ? <button type="button" className="article-source" title={t("card.openFeed", { feed: row.feed_title })}
              onClick={(event) => { event.stopPropagation(); onSource(row.feed_id); }}>{row.feed_title}</button>
              : <span className="article-source" title={row.feed_title}>{row.feed_title}</span>}
            {row.published_at != null ? <span className="meta-sep">·</span> : null}
            {row.published_at != null ? <RelativeTime className="article-time" ms={row.published_at} /> : null}
            {representative ? <><span className="meta-sep">·</span><span className="article-rep">{t("card.representative")}</span></> : null}
            {showReasons && row.interest_reason ? <span className="meta-tag interest">{formatReason(row.interest_reason).replace(/^\s*·\s*/, "")}</span> : null}
            {showReasons && learned ? <span className="meta-tag learned" title={row.rank_reasons?.map(formatReason).join(t("card.reasonSeparator"))}>{formatReason(learned)}</span> : null}
            {showReasons && row.noisy ? <span className="meta-tag">{t("card.noisy")}</span> : null}
            {details && (row.spot_check || review) ? <span className="meta-tag" title={t("card.spotCheckTitle")}>{t("card.spotCheck")}</span> : null}
            {row.starred ? <span className="meta-tag starred"><Icon name="star" size={11} filled />{t("card.starred")}</span> : null}
          </div>
          {showReasons && score !== 0 ? <span className={`score ${scoreClass}`} title={t("card.scoreTitle")}>{score > 0 ? "+" : ""}{formatReasonNumber(score)}</span> : null}
          <div className="acts" role="group" aria-label={t("card.actions")}
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") event.stopPropagation();
            }}>
            {onStar ? (
              <button type="button" className={`star-btn${row.starred ? " on" : ""}`}
                title={t(row.starred ? "card.unstar" : "card.star")}
                aria-label={t(row.starred ? "card.unstar" : "card.star")} aria-pressed={row.starred}
                onClick={() => onStar(row)}><Icon name="star" size={15} filled={row.starred} /></button>
            ) : null}
            <button type="button" className="cite-btn" title={t("card.citeTitle")} onClick={() => onCite(row.id)}><Icon name="quote" size={14} />{t("card.cite")}</button>
            <button type="button" className="rate up" title={t("card.like")} aria-label={t("card.like")} onClick={() => onRate(row, true)}><ThumbIcon /></button>
            <button type="button" className="rate down" title={t("card.dislike")} aria-label={t("card.dislike")} onClick={() => onRate(row, false)}><ThumbIcon down /></button>
          </div>
        </div>
        {details && review && onReview ? <SpotCheckReveal review={review} threshold={hideThreshold} busy={reviewBusy} onAction={onReview} /> : null}
      </div>
    </article>
  );
}
