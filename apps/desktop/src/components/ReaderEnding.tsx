import type { ArticleView, ReaderView } from "../lib/api";
import { Icon } from "./Icon";
import { RelativeTime } from "./RelativeTime";
import { ThumbIcon } from "./ThumbIcon";

export type NextArticle = Pick<ArticleView, "title" | "feed_title"> & { published_at?: number | null };

/**
 * 文末只接阅读器现有动作；下一篇与 J 使用同一个导航回调。焦点列表与搜索结果共用一张卡：
 * 搜索多一行「第 n / 共 m 条」，读完时换成完成态（搜索另给「回到搜索」）。
 */
export function ReaderEnding({ reader, onRate, onStar, onCite, showNext, hasNext, nextArticle, onNext, searchPosition, onReturnToSearch }: {
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
}) {
  const search = !!searchPosition;
  const done = !hasNext;
  const label = done ? "已经到底了" : search ? "下一条结果" : "下一篇";
  const hint = done ? (search ? "返回后保留搜索条件，并选中当前这条。" : "按 Esc 回到焦点列表，仍选中当前这篇。")
    : "读到这里了。按 Space 聚焦下一篇，或按 J 继续。";
  return <footer className="reader-ending">
    <div className="reader-finis" aria-hidden="true"><i /></div>
    <div className="reader-ending-actions" role="group" aria-label="文章评价">
      <span>这篇推荐得准吗？</span>
      <button type="button" aria-pressed={reader.feedback === 1} onClick={() => onRate(true)}><ThumbIcon />赞</button>
      <button type="button" aria-pressed={reader.feedback === -1} onClick={() => onRate(false)}><ThumbIcon down />踩</button>
      <button type="button" aria-pressed={reader.starred} disabled={!onStar} onClick={onStar}><Icon name="star" size={14} filled={reader.starred} />收藏</button>
      <button type="button" onClick={onCite}><Icon name="quote" size={14} />引用到对话</button>
    </div>
    {showNext ? <>
      <button type="button" className="reader-next" data-has-next={hasNext ? "true" : "false"} aria-disabled={done}
        onClick={hasNext ? onNext : undefined}>
        <span className="reader-next-copy">
          <span className="reader-next-label">{label}{done ? null : <kbd>J</kbd>}
            {searchPosition ? <span className="reader-next-position">第 {searchPosition.index + 1} / 共 {searchPosition.total} 条</span> : null}</span>
          <strong>{done ? (search ? "搜索结果读完了" : "今天的焦点读完了") : nextArticle?.title || label}</strong>
          {done ? <span className="reader-next-meta">{search ? "回去换个关键词，或再看看前面的结果。" : "可以回到列表，看看收藏或其他来源。"}</span>
            : nextArticle?.feed_title ? <span className="reader-next-meta">{nextArticle.feed_title}{nextArticle.published_at != null ? <> · <RelativeTime ms={nextArticle.published_at} /></> : null}</span> : null}
        </span>
        <span className={`reader-next-arrow${done ? " complete-icon" : ""}`}><Icon name={done ? "check" : "arrowRight"} size={16} /></span>
      </button>
      {done && search && onReturnToSearch ? <button type="button" className="btn return-search" onClick={onReturnToSearch}><Icon name="arrowLeft" size={14} />回到搜索 <kbd>Esc</kbd></button> : null}
      <p className="reader-ending-hint">{hint}</p>
    </> : null}
  </footer>;
}
