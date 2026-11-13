import { useTranslation } from "../i18n";
import type { ArticleView, ReaderView, RelatedArticleView } from "../lib/api";
import { Icon } from "./Icon";
import { RelativeTime } from "./RelativeTime";
import { ThumbIcon } from "./ThumbIcon";

export type NextArticle = Pick<ArticleView, "title" | "feed_title"> & { published_at?: number | null };

/** 文末相关阅读：本篇「标题 + 摘要」的近邻，排除同一组相近报道；放在下一篇卡之后，不抢下一篇的位置。 */
function RelatedReading({ items, onOpen }: { items: RelatedArticleView[]; onOpen: (item: RelatedArticleView) => void }) {
  const { t } = useTranslation("reader");
  return <section className="reader-related" aria-labelledby="reader-related-title">
    <h3 className="reader-related-head" id="reader-related-title"><strong>{t("ending.related.title")}</strong><span>{t("ending.related.hint")}</span></h3>
    <ul className="reader-related-list">
      {items.map(item => <li key={item.article_id}>
        <button type="button" className="reader-related-row" data-read={item.read || undefined} onClick={() => onOpen(item)}>
          <span className="reader-related-title">{item.title}</span>
          <span className="reader-related-meta">{item.feed_title}{item.published_at != null ? <> · <RelativeTime ms={item.published_at} /></> : null}</span>
          {item.read ? <span className="reader-related-tag">{t("ending.related.read")}</span> : null}
        </button>
      </li>)}
    </ul>
  </section>;
}

/**
 * 文末只接阅读器现有动作；下一篇与 J 使用同一个导航回调。焦点列表与搜索结果共用一张卡：
 * 搜索多一行「第 n / 共 m 条」，读完时换成完成态（搜索另给「回到搜索」）。
 */
export function ReaderEnding({ reader, onRate, onStar, onCite, showNext, hasNext, nextArticle, onNext, searchPosition, onReturnToSearch, related, onOpenRelated }: {
  reader: ReaderView;
  onRate: (up: boolean) => void;
  onStar?: () => void;
  onCite: () => void;
  showNext: boolean;
  hasNext: boolean;
  nextArticle?: NextArticle;
  onNext?: () => void;
  searchPosition?: { index: number; total: number };
  onReturnToSearch?: () => void;
  related?: RelatedArticleView[];
  onOpenRelated?: (item: RelatedArticleView) => void;
}) {
  const { t } = useTranslation("reader");
  const search = !!searchPosition;
  const done = !hasNext;
  const label = done ? t("ending.next.end") : search ? t("ending.next.result") : t("ending.next.article");
  const hint = done ? (search ? t("ending.hint.doneSearch") : t("ending.hint.doneFocus"))
    : t("ending.hint.next");
  return <footer className="reader-ending">
    <div className="reader-finis" aria-hidden="true"><i /></div>
    <div className="reader-ending-actions" role="group" aria-label={t("ending.rating.label")}>
      <span>{t("ending.rating.prompt")}</span>
      <button type="button" aria-pressed={reader.feedback === 1} onClick={() => onRate(true)}><ThumbIcon />{t("ending.rating.up")}</button>
      <button type="button" aria-pressed={reader.feedback === -1} onClick={() => onRate(false)}><ThumbIcon down />{t("ending.rating.down")}</button>
      <button type="button" aria-pressed={reader.starred} disabled={!onStar} onClick={onStar}><Icon name="star" size={14} filled={reader.starred} />{t("ending.star")}</button>
      <button type="button" onClick={onCite}><Icon name="quote" size={14} />{t("ending.cite")}</button>
    </div>
    {showNext ? <>
      <button type="button" className="reader-next" data-has-next={hasNext ? "true" : "false"} aria-disabled={done}
        onClick={hasNext ? onNext : undefined}>
        <span className="reader-next-copy">
          <span className="reader-next-label">{label}{done ? null : <kbd>J</kbd>}
            {searchPosition ? <span className="reader-next-position">{t("ending.position", { index: searchPosition.index + 1, total: searchPosition.total })}</span> : null}</span>
          <strong>{done ? (search ? t("ending.done.search") : t("ending.done.focus")) : nextArticle?.title || label}</strong>
          {done ? <span className="reader-next-meta">{search ? t("ending.doneMeta.search") : t("ending.doneMeta.focus")}</span>
            : nextArticle?.feed_title ? <span className="reader-next-meta">{nextArticle.feed_title}{nextArticle.published_at != null ? <> · <RelativeTime ms={nextArticle.published_at} /></> : null}</span> : null}
        </span>
        <span className={`reader-next-arrow${done ? " complete-icon" : ""}`}><Icon name={done ? "check" : "arrowRight"} size={16} /></span>
      </button>
      {done && search && onReturnToSearch ? <button type="button" className="btn return-search" onClick={onReturnToSearch}><Icon name="arrowLeft" size={14} />{t("ending.returnToSearch")} <kbd>Esc</kbd></button> : null}
      <p className="reader-ending-hint">{hint}</p>
    </> : null}
    {related?.length && onOpenRelated ? <RelatedReading items={related} onOpen={onOpenRelated} /> : null}
  </footer>;
}
