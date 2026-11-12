import { SourceAvatar } from "../../components/ArticleCard";
import { Icon } from "../../components/Icon";
import type { ClusterArticleView } from "../../lib/api";
import { formatPublished } from "../../lib/time";
import { boldSegments, type ClusterSummaryEntry } from "./clusterSummaries";

/** 来源小标签：头像 + 来源名（过长在尾部截断，悬停看全名），点开读这篇原文。 */
function SourceChip({ source, onOpen }: { source: ClusterArticleView; onOpen: (id: number) => void }) {
  return (
    <button type="button" className="cluster-chip" title={`打开 ${source.feed_title} 的报道：${source.title}`} onClick={() => onOpen(source.article_id)}>
      <SourceAvatar title={source.feed_title} iconHref={source.icon_href} size={14} /><span className="cluster-chip-name">{source.feed_title}</span>
    </button>
  );
}

/** 估算用量写成「约 2k」这种整数或一位小数的千位说法。 */
const tokens = (n: number) => `约 ${n >= 1000 ? `${Number((n / 1000).toFixed(1))}k` : n} token`;

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
  const { view, generating, error } = entry;
  const count = view?.members.length ?? 0;
  if (generating) {
    return (
      <div className="cluster-summary" aria-busy="true">
        <div className="cluster-summary-head"><strong><Icon name="sparkles" size={13} />正在汇总{count ? ` ${count} 篇` : ""}报道…</strong></div>
        <div className="cluster-summary-skeleton" aria-hidden="true"><span /><span /><span /><span /></div>
      </div>
    );
  }
  if (!view) {
    return error ? (
      <p className="cluster-summary-row" role="alert">
        <span className="cluster-summary-text">{error}</span>
        <button type="button" className="btn sm" onClick={onRetryLoad}>重试</button>
      </p>
    ) : null;
  }
  const failure = error ? (
    <p className="cluster-summary-error" role="alert">
      <Icon name="warning" size={13} /><span>{error}</span>
      {view.model_ready ? <button type="button" className="cluster-summary-link" onClick={onGenerate}>重试</button> : null}
    </p>
  ) : null;
  const pickModel = onManageModels ? <button type="button" className="cluster-summary-link" onClick={onManageModels}>去选模型</button> : null;

  if (view.state === "none") {
    // 没配摘要模型：不调用模型；说清还差什么，下面列其余各篇已有的一句话（没有一句话的、代表文章不列，免得和成员行、卡片重复）。
    if (!view.model_ready) {
      const briefs = view.members.filter((member) => member.one_liner && member.article_id !== representativeId);
      return (
        <div className="cluster-summary">
          <div className="cluster-summary-head">
            <span className="cluster-summary-text"><b>汇总这 {count} 篇报道</b><small>生成综述要先选摘要模型</small></span>
            {onManageModels ? <button type="button" className="btn sm" onClick={onManageModels}>去选模型</button> : null}
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
          <b>汇总这 {count} 篇报道</b>
          <small>写一段综述，标出各家补充与说法不一 · 用摘要模型，{tokens(view.estimate_tokens)}</small>
        </span>
        <button type="button" className="btn primary sm" onClick={onGenerate}><Icon name="sparkles" size={13} />生成综述</button>
        {failure}
      </div>
    );
  }

  const stale = view.state === "stale" ? (
    <div className="cluster-summary-stale">
      <Icon name="sparkles" size={13} />
      <span>{view.fresh.length
        ? `新增 ${view.fresh.length} 篇：${view.fresh.map((item) => `${item.feed_title} · ${formatPublished(item.published_at)}`).join("、")}`
        : "这组报道有更新"}</span>
      <span className="cluster-summary-spacer" />
      {view.model_ready ? <button type="button" className="btn sm" onClick={onGenerate}>更新综述</button> : pickModel}
    </div>
  ) : null;
  return (
    <div className="cluster-summary">
      {failure}
      {stale}
      <div className="cluster-summary-head">
        <strong><Icon name="sparkles" size={13} />综述</strong>
        <span>依据 {view.basis} 篇{view.generated_at != null ? ` · ${formatPublished(view.generated_at)}生成` : ""}</span>
        <span className="cluster-summary-spacer" />
        <button type="button" className="btn ghost sm" onClick={() => onCopy(overviewText(view.overview))}><Icon name="copy" size={13} />复制</button>
        {view.model_ready ? (
          <button type="button" className="btn ghost sm" title="重新生成会再用一次摘要模型" onClick={onGenerate}><Icon name="refresh" size={13} />重新生成</button>
        ) : null}
      </div>
      <p className="cluster-overview">
        {boldSegments(view.overview).map((part, i) => (part.bold ? <b key={i}>{part.text}</b> : <span key={i}>{part.text}</span>))}
      </p>
      {view.additions.length ? <>
        <p className="cluster-summary-sub">各家补充</p>
        <ul className="cluster-notes">
          {view.additions.map((note, i) => (
            <li key={i}>{note.sources.map((source) => <SourceChip key={source.article_id} source={source} onOpen={onOpen} />)}<span>{note.text}</span></li>
          ))}
        </ul>
      </> : null}
      {view.conflicts.length ? <>
        <p className="cluster-summary-sub">说法不一</p>
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
