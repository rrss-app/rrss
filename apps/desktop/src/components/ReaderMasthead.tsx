import { useMemo } from "react";
import type { ArticleView, ReaderView } from "../lib/api";
import { openOriginal } from "../lib/signals";
import { useTranslation } from "../i18n";
import { readingMinutes } from "../lib/readingTime";
import { SourceAvatar } from "./ArticleCard";
import { RelativeTime } from "./RelativeTime";
import { Icon } from "./Icon";

export function ReaderMasthead({ reader, article, title, originalTitle, web = false, onSource, sourceOpen }: { reader: ReaderView; article?: ArticleView; title: string; originalTitle?: string; web?: boolean; onSource?: () => void; sourceOpen?: boolean }) {
  const { t } = useTranslation("reader");
  const parts = reader.meta.split(" · ");
  const source = reader.feed_title || article?.feed_title || parts[0];
  const badge = <><SourceAvatar title={source} size={18} iconHref={article?.feed_icon_href || reader.feed_icon_href} /><span>{source}</span></>;
  // ReaderView 的历史 meta 是本地 YYYY-MM-DD HH:mm；列表入口优先用结构化时间。
  const fallbackTime = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/.test(parts[1] ?? "") ? new Date(parts[1].replace(" ", "T")).getTime() : NaN;
  const published = article?.published_at ?? (Number.isFinite(fallbackTime) ? fallbackTime : null);
  const minutes = useMemo(() => readingMinutes(reader.document), [reader.document]);
  let domain = "";
  try { domain = new URL(reader.url).hostname; } catch { /* 没有原文地址时不造域名。 */ }
  return <>
    {source && !web ? onSource ? <button type="button" className="reader-source" aria-expanded={!!sourceOpen} onClick={onSource}>{badge}<Icon name="chevronRight" size={12} /></button> : <div className="reader-source">{badge}</div> : null}
    <h1 className="reader-title">{title}</h1>
    {originalTitle ? <p className="reader-original-title">{originalTitle}</p> : null}
    <div className="reader-meta-bar">
      {[
        published != null ? <RelativeTime key="time" ms={published} /> : parts[1] ? <span key="time">{parts[1]}</span> : null,
        minutes && !web ? <span key="length">{t("masthead.minutes", { count: minutes })}</span> : null,
        domain && !web ? <a key="domain" href={reader.url} onClick={event => { event.preventDefault(); void openOriginal(reader.id, reader.url); }}>{domain}</a> : null,
      ].filter(Boolean).flatMap((item, index) => index ? [<span key={`sep-${index}`} className="reader-meta-sep" aria-hidden="true">·</span>, item] : [item])}
    </div>
  </>;
}
