import { useLayoutEffect, useRef, useState } from "react";
import type { RuleCandidateView } from "../lib/api";
import { ruleConditionLabel } from "../lib/ruleLabels";
import { useTranslation } from "../i18n";
import { Icon } from "./Icon";

/** 点踩后可撤销的时长（阶段 11 · 踩的反馈）。 */
export const DISLIKE_HOLD_MS = 5000;

export type DislikeRules = { loading: boolean; candidates: RuleCandidateView[]; error: string; busy: boolean };
/** 一张点踩后还在撤销窗口里的卡片。倒计时由焦点会话持有，卡片卸载重挂（虚拟化）也不丢。 */
export type DislikeState = {
  /** 正在走的截止时刻；暂停时为 null。 */
  deadline: number | null;
  /** 暂停时剩下的毫秒。 */
  remainingMs: number;
  /** 每次从头计时加一，卡片据此重启倒计时动画。 */
  run: number;
  /** 正在淡出、等补位。 */
  leaving: boolean;
  /** 展开的「加一条规则…」；展开期间不计时。 */
  rules: DislikeRules | null;
};
export type DislikeAction =
  | { kind: "undo" }
  | { kind: "hold"; held: boolean }
  | { kind: "rules"; open: boolean }
  | { kind: "rule"; candidate: RuleCandidateView; feedScope: boolean };

/** 卡片虚化后盖在上面的一行：已记下 + 撤销 + 加一条规则…，底部细线倒计时。 */
export function DislikeVeil({ state, onAction }: { state: DislikeState; onAction: (action: DislikeAction) => void }) {
  const { t } = useTranslation("focus");
  const line = useRef<HTMLDivElement>(null);
  const hover = useRef({ pointer: false, focus: false, held: false });
  const hold = (patch: Partial<{ pointer: boolean; focus: boolean }>) => {
    const was = hover.current.held;
    const next = { ...hover.current, ...patch };
    next.held = next.pointer || next.focus;
    hover.current = next;
    if (next.held !== was) onAction({ kind: "hold", held: next.held });
  };
  // 点踩的按钮随正文一起失效；键盘焦点交给这一行，Tab 再进撤销。焦点停在这一行本身不暂停计时。
  useLayoutEffect(() => {
    const active = document.activeElement;
    const article = line.current?.closest("article");
    if (!active || active === document.body || article?.contains(active)) line.current?.focus({ preventScroll: true });
  }, []);
  const remaining = state.deadline === null ? state.remainingMs : Math.max(0, state.deadline - Date.now());
  return <>
    <div ref={line} className="dismiss-veil" role="status" tabIndex={-1}>
      <span className="dismiss-note"><Icon name="check" size={16} />{t("dislike.recorded")}</span>
      <span className="dismiss-actions"
        onPointerEnter={() => hold({ pointer: true })} onPointerLeave={() => hold({ pointer: false })}
        onFocus={() => hold({ focus: true })}
        onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) hold({ focus: false }); }}>
        <button type="button" className="dismiss-undo" onClick={() => onAction({ kind: "undo" })}>{t("dislike.undo")}</button>
        <button type="button" className="dismiss-more" aria-expanded={!!state.rules}
          onClick={() => onAction({ kind: "rules", open: !state.rules })}>{t("dislike.addRule")}</button>
      </span>
    </div>
    {state.rules ? null : <div className="dismiss-countdown" aria-hidden="true">
      <i key={state.run} style={{
        animationDuration: `${DISLIKE_HOLD_MS}ms`,
        animationDelay: `-${DISLIKE_HOLD_MS - remaining}ms`,
        animationPlayState: state.deadline === null ? "paused" : "running",
      }} />
    </div>}
  </>;
}

/** 卡片内展开的「加一条规则」：候选与排序来自 `rule_candidates`。 */
export function DislikeRulesPanel({ rules, onAction }: { rules: DislikeRules; onAction: (action: DislikeAction) => void }) {
  const { t } = useTranslation("focus");
  return (
    <div className="dismiss-rules" role="group" aria-labelledby="dismiss-rules-title" aria-busy={rules.loading || rules.busy}>
      <div className="dismiss-rules-head">
        <strong id="dismiss-rules-title">{t("dislike.rulesTitle")}</strong>
        <small>{t("dislike.rulesHint")}</small>
      </div>
      {rules.error ? <p className="notice error" role="alert">{rules.error}</p> : null}
      {rules.loading ? <p className="muted">{t("dislike.rulesLoading")}</p>
        : rules.candidates.length ? <RuleCandidateList candidates={rules.candidates} busy={rules.busy}
          onPick={(candidate, feedScope) => onAction({ kind: "rule", candidate, feedScope })} />
        : rules.error ? null : <p className="muted">{t("dislike.rulesEmpty")}</p>}
      <div className="dismiss-rules-foot">
        <button type="button" className="btn sm ghost" disabled={rules.busy} onClick={() => onAction({ kind: "rules", open: false })}>{t("dislike.rulesClose")}</button>
      </div>
    </div>
  );
}

/** 踩的规则候选：可能误伤的默认折叠；没有其他候选时直接列出。阅读器训练条与卡片共用。 */
export function RuleCandidateList({ candidates, busy = false, onPick }: {
  candidates: RuleCandidateView[];
  busy?: boolean;
  onPick: (candidate: RuleCandidateView, feedScope: boolean) => void;
}) {
  const { t } = useTranslation("focus");
  const [showRisky, setShowRisky] = useState(false);
  const safe = candidates.filter((c) => c.caution === "none");
  const risky = candidates.filter((c) => c.caution !== "none");
  const folded = safe.length > 0 && risky.length > 0;
  const row = (c: RuleCandidateView) => (
    <div key={c.dimension + c.pattern} className="rule-candidate" data-caution={c.caution}>
      <div className="rule-candidate-what">
        <span>{t("trainingBar.scoreDown", { label: ruleConditionLabel(c, "focus") })}</span>
        {c.best ? <span className="rule-candidate-best">{t("dislike.best")}</span> : null}
        <small>
          {c.caution === "whole_feed" ? <><em>{t("dislike.wholeFeed")}</em>{t("dislike.separator")}</> : null}
          {t("dislike.reach", { count: c.matched_30d, read: c.read_30d })}
          {c.caution === "reads" ? <>{t("dislike.separator")}<em>{t("dislike.mayHit")}</em></> : null}
        </small>
      </div>
      <div className="rule-candidate-pick">
        <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={() => onPick(c, false)}>{t("trainingBar.global")}</button>
        <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={() => onPick(c, true)}>{t("trainingBar.feedOnly")}</button>
      </div>
    </div>
  );
  return (
    <div className="rule-candidates">
      <div className="rule-candidate-list">{(folded ? safe : candidates).map(row)}</div>
      {folded ? <button type="button" className="rule-candidates-more" aria-expanded={showRisky} onClick={() => setShowRisky(!showRisky)}>
        <Icon name="chevronRight" size={14} />{t("dislike.risky", { count: risky.length })}
      </button> : null}
      {folded && showRisky ? <div className="rule-candidate-list">{risky.map(row)}</div> : null}
    </div>
  );
}
