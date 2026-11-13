import { SourceAvatar } from "../../components/ArticleCard";
import { Icon } from "../../components/Icon";
import { useTranslation } from "../../i18n";
import type { ClusterArticleView } from "../../lib/api";
import { formatPublished } from "../../lib/time";
import { boldSegments, type ClusterSummaryEntry } from "./clusterSummaries";

/** 来源小标签：头像 + 来源名（过长在尾部截断，悬停看全名），点开读这篇原文。 */
function SourceChip({ source, onOpen }: { source: ClusterArticleView; onOpen: (id: number) => void }) {
  const { t } = useTranslation("focus");
  return (
    <button type="button" className="cluster-chip" title={t("cluster.openSource", { feed: source.feed_title, title: source.title })} onClick={() => onOpen(source.article_id)}>
      <SourceAvatar title={source.feed_title} iconHref={source.icon_href} size={14} /><span className="cluster-chip-name">{source.feed_title}</span>
    </button>
  );
}

/** 估算用量写成「2k」这种整数或一位小数的千位说法。 */
const tokenAmount = (n: number) => (n >= 1000 ? `${Number((n / 1000).toFixed(1))}k` : `${n}`);

/** 综述正文：只渲染 `**粗体**`，其余纯文本。 */
export function overviewText(text: string): string {
  return boldSegments(text).map((part) => part.text).join("");
}

/**
 * 面板内的综述（阶段 14 · 相近报道成组与汇总）：未生成、生成中、已生成、有新报道、没配摘要模型五种状态，
 * 生成失败时在原位写原因并给「重试」，旧综述照常显示。放在代表文章下、成员列表上方。
 */
export function ClusterSummary({ entry, representativeId, onGenerate, onRetryLoad, onOpen, onManageModels, onCopy }: {
  entry: ClusterSummaryEntry;
  /** 代表文章的卡片就在上方，摘要已经看得到，一句话列表里不再重复它。 */
  representativeId?: number;
  onGenerate: () => void;
  onRetryLoad: () => void;
  onOpen: (id: number) => void;
  onManageModels?: () => void;
  onCopy: (text: string) => void;
}) {
  const { t } = useTranslation("focus");
  const { view, generating, error } = entry;
  const count = view?.members.length ?? 0;
  if (generating) {
    return (
      <div className="cluster-summary" aria-busy="true">
        <div className="cluster-summary-head"><strong><Icon name="sparkles" size={13} />{count ? t("cluster.summarizingCount", { count }) : t("cluster.summarizing")}</strong></div>
        <div className="cluster-summary-skeleton" aria-hidden="true"><span /><span /><span /><span /></div>
      </div>
    );
  }
  if (!view) {
    return error ? (
      <p className="cluster-summary-row" role="alert">
        <span className="cluster-summary-text">{error}</span>
        <button type="button" className="btn sm" onClick={onRetryLoad}>{t("cluster.retry")}</button>
      </p>
    ) : null;
  }
  const failure = error ? (
    <p className="cluster-summary-error" role="alert">
      <Icon name="warning" size={13} /><span>{error}</span>
      {view.model_ready ? <button type="button" className="cluster-summary-link" onClick={onGenerate}>{t("cluster.retry")}</button> : null}
    </p>
  ) : null;
  const pickModel = onManageModels ? <button type="button" className="cluster-summary-link" onClick={onManageModels}>{t("cluster.pickModel")}</button> : null;

  if (view.state === "none") {
    // 没配摘要模型：不调用模型；说清还差什么，下面列其余各篇已有的一句话（没有一句话的、代表文章不列，免得和成员行、卡片重复）。
    if (!view.model_ready) {
      const briefs = view.members.filter((member) => member.one_liner && member.article_id !== representativeId);
      return (
        <div className="cluster-summary">
          <div className="cluster-summary-head">
            <span className="cluster-summary-text"><b>{t("cluster.summarizeAll", { count })}</b><small>{t("cluster.needModel")}</small></span>
            {onManageModels ? <button type="button" className="btn sm" onClick={onManageModels}>{t("cluster.pickModel")}</button> : null}
          </div>
          {briefs.length ? (
            <ul className="cluster-notes">
              {briefs.map((member) => (
                <li key={member.article_id}><SourceChip source={member} onOpen={onOpen} /><span>{member.one_liner}</span></li>
              ))}
            </ul>
          ) : null}
        </div>
      );
    }
    return (
      <div className="cluster-summary-row">
        <span className="cluster-summary-text">
          <b>{t("cluster.summarizeAll", { count })}</b>
          <small>{t("cluster.generateHint", { tokens: t("cluster.tokens", { n: tokenAmount(view.estimate_tokens) }) })}</small>
        </span>
        <button type="button" className="btn primary sm" onClick={onGenerate}><Icon name="sparkles" size={13} />{t("cluster.generate")}</button>
        {failure}
      </div>
    );
  }

  const stale = view.state === "stale" ? (
    <div className="cluster-summary-stale">
      <Icon name="sparkles" size={13} />
      <span>{view.fresh.length
        ? t("cluster.fresh", { count: view.fresh.length, items: view.fresh.map((item) => `${item.feed_title} · ${formatPublished(item.published_at)}`).join(t("cluster.listSeparator")) })
        : t("cluster.updated")}</span>
      <span className="cluster-summary-spacer" />
      {view.model_ready ? <button type="button" className="btn sm" onClick={onGenerate}>{t("cluster.update")}</button> : pickModel}
    </div>
  ) : null;
  return (
    <div className="cluster-summary">
      {failure}
      {stale}
      <div className="cluster-summary-head">
        <strong><Icon name="sparkles" size={13} />{t("cluster.overview")}</strong>
        <span>{view.generated_at != null ? t("cluster.basisGenerated", { count: view.basis, time: formatPublished(view.generated_at) }) : t("cluster.basis", { count: view.basis })}</span>
        <span className="cluster-summary-spacer" />
        <button type="button" className="btn ghost sm" onClick={() => onCopy(overviewText(view.overview))}><Icon name="copy" size={13} />{t("cluster.copy")}</button>
        {view.model_ready ? (
          <button type="button" className="btn ghost sm" title={t("cluster.regenerateHint")} onClick={onGenerate}><Icon name="refresh" size={13} />{t("cluster.regenerate")}</button>
        ) : null}
      </div>
      <p className="cluster-overview">
        {boldSegments(view.overview).map((part, i) => (part.bold ? <b key={i}>{part.text}</b> : <span key={i}>{part.text}</span>))}
      </p>
      {view.additions.length ? <>
        <p className="cluster-summary-sub">{t("cluster.additions")}</p>
        <ul className="cluster-notes">
          {view.additions.map((note, i) => (
            <li key={i}>{note.sources.map((source) => <SourceChip key={source.article_id} source={source} onOpen={onOpen} />)}<span>{note.text}</span></li>
          ))}
        </ul>
      </> : null}
      {view.conflicts.length ? <>
        <p className="cluster-summary-sub">{t("cluster.conflicts")}</p>
        <ul className="cluster-conflicts">
          {view.conflicts.map((note, i) => (
            <li key={i}>
              <Icon name="warning" size={13} />
              <span>{note.text}</span>
              <span className="cluster-conflict-refs">{note.sources.map((source) => <SourceChip key={source.article_id} source={source} onOpen={onOpen} />)}</span>
            </li>
          ))}
        </ul>
      </> : null}
    </div>
  );
}
