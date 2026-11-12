import type { ArticleView } from "../lib/api";

export type SpotAction = "disable" | "halve" | "allow" | "keep" | "undo";
export type SpotReview = { article: ArticleView; up: boolean; result?: string; undo?: () => Promise<unknown> };

export function SpotCheckReveal({ review, threshold, busy, onAction }: {
  review: SpotReview;
  threshold: number;
  busy: boolean;
  onAction: (action: SpotAction) => void;
}) {
  const { article, up, result, undo } = review;
  const reasons = article.hide_reasons ?? [];
  return <div className="spot-check-reveal" role="status" data-tone={up ? undefined : "ok"}
    onClick={event => event.stopPropagation()} onKeyDown={event => event.stopPropagation()}>
    {result && up ? <div className="spot-check-result">这是一次抽查：{result}</div> : <div>
      这是一次抽查：它原本会被 {reasons.length ? reasons.map((reason, index) => <span key={reason.rule_id}>
        {index ? "、" : ""}<strong>{reason.label}</strong>{up ? `（${reason.weight}）` : ""}
      </span>) : "隐藏规则"} 隐藏，{up ? `总分 ${article.score}，隐藏阈值 ${threshold}。` : "看来规则没判错。"}
    </div>}
    <div className="spot-check-options">
      {result ? undo ? <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={() => onAction("undo")}>撤销</button> : null : up ? <>
        {reasons.length > 0 ? <>
          <button type="button" className="btn sm" disabled={busy} onClick={() => onAction("disable")}>停用这条规则</button>
          <button type="button" className="btn sm" disabled={busy} onClick={() => onAction("halve")}>权重减半</button>
          <button type="button" className="btn sm" disabled={busy} onClick={() => onAction("allow")}>对 {article.feed_title} 放行</button>
        </> : null}
        <button type="button" className="btn sm ghost" disabled={busy} onClick={() => onAction("keep")}>保持不变</button>
      </> : null}
    </div>
    <small>{up ? "规则不会自动修改；选一项才生效，随时可以撤销。" : "这次反馈记为隐藏正确。"}</small>
  </div>;
}
