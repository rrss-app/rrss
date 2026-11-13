import type { Ref } from "react";
import type { CitationView } from "../lib/api";
import { useTranslation } from "../i18n";
import { Icon } from "./Icon";

/** 托盘、待发草稿与消息共用小签；只有消息 / 草稿传入打开动作。 */
export function CitationChip({ citation, onOpen, onRemove, removeLabel, removeDisabled, isNew, className = "", ref }: {
  citation: CitationView;
  onOpen?: (id: number, anchor?: { blockId: number }) => void;
  onRemove?: () => void;
  removeLabel?: string;
  removeDisabled?: boolean;
  isNew?: boolean;
  className?: string;
  ref?: Ref<HTMLSpanElement>;
}) {
  const { t } = useTranslation("reader");
  const article = citation.kind === "article";
  const text = <>
    {article ? <span className="citation-full">{t("citation.full")}</span> : citation.kind === "digest" ? <span className="citation-full">{t("citation.digest")}</span> : null}
    <span className={`citation-text${article ? "" : " citation-excerpt"}`}>{article ? t("citation.articleTitle", { title: citation.title }) : t("citation.excerpt", { excerpt: citation.excerpt })}</span>
  </>;
  return (
    <span ref={ref} className={`chip citation-chip ${className}`} data-new={isNew || undefined} title={article ? citation.title : citation.excerpt}
      data-article-id={citation.article_id > 0 ? citation.article_id : undefined}>
      {onOpen ? <button
        type="button"
        className="citation-open"
        title={citation.title}
        aria-label={t("citation.open", { title: citation.title })}
        disabled={citation.article_id <= 0}
        onClick={() => onOpen?.(citation.article_id, citation.kind === "span" && Number.isSafeInteger(citation.block_id) && citation.block_id > 0 ? { blockId: citation.block_id } : undefined)}
      >
        {text}
      </button> : text}
      {onRemove ? <button type="button" className="citation-remove" aria-label={removeLabel ?? t("citation.remove")} disabled={removeDisabled} onClick={onRemove}><Icon name="close" size={12} /></button> : null}
    </span>
  );
}
