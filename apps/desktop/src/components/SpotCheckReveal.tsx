import type { ArticleView } from "../lib/api";
import { Trans, useTranslation } from "../i18n";

export type SpotAction = "disable" | "halve" | "allow" | "keep" | "undo";
export type SpotReview = { article: ArticleView; up: boolean; result?: string; undo?: () => Promise<unknown> };

export function SpotCheckReveal({ review, threshold, busy, onAction }: {
  review: SpotReview;
  threshold: number;
  busy: boolean;
  onAction: (action: SpotAction) => void;
}) {
  const { t } = useTranslation("focus");
  const { article, up, result, undo } = review;
  const reasons = article.hide_reasons ?? [];
  return <div className="spot-check-reveal" role="status" data-tone={up ? undefined : "ok"}
    onClick={event => event.stopPropagation()} onKeyDown={event => event.stopPropagation()}>
    {result && up ? <div className="spot-check-result">{t("spot.result", { result })}</div> : <div>
      <Trans t={t} i18nKey={up ? "spot.wouldHideUp" : "spot.wouldHideDown"} values={{ score: article.score, threshold }}
        components={{ reasons: <>{reasons.length ? reasons.map((reason, index) => <span key={reason.rule_id}>
          {index ? t("spot.reasonSeparator") : ""}<strong>{reason.label}</strong>{up ? t("spot.weight", { weight: reason.weight }) : ""}
        </span>) : t("spot.fallbackRule")}</> }} />
    </div>}
    <div className="spot-check-options">
      {result ? undo ? <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={() => onAction("undo")}>{t("spot.undo")}</button> : null : up ? <>
        {reasons.length > 0 ? <>
          <button type="button" className="btn sm" disabled={busy} onClick={() => onAction("disable")}>{t("spot.disable")}</button>
          <button type="button" className="btn sm" disabled={busy} onClick={() => onAction("halve")}>{t("spot.halve")}</button>
          <button type="button" className="btn sm" disabled={busy} onClick={() => onAction("allow")}>{t("spot.allow", { feed: article.feed_title })}</button>
        </> : null}
        <button type="button" className="btn sm ghost" disabled={busy} onClick={() => onAction("keep")}>{t("spot.keep")}</button>
      </> : null}
    </div>
    <small>{t(up ? "spot.noteUp" : "spot.noteDown")}</small>
  </div>;
}
