import { forwardRef, useImperativeHandle, useRef, useState, type CSSProperties } from "react";
import { TrainingBar } from "../../components/TrainingBar";
import { api, type ArticleView, type FeatureView, type RatingUndoView, type RuleCandidateView } from "../../lib/api";
import { useTranslation } from "../../i18n";
import { trackEvent } from "../../lib/signals";
import { formatNotice } from "../../lib/notice";
import type { ArticleWriteOrder } from "../../lib/articleWriteOrder";
import type { FocusNotice, FocusSession } from "./focusSession";

/** 点赞、阅读器里点踩后的提示时长，也是撤销窗口（阶段 11 · 赞与踩的反馈）。 */
const QUIET_RATING_MS = 6000;
/** 取消赞时撤销用的「之前没有评分」。 */
const NO_PREVIOUS = { previous: 0, previous_at: 0, suggested_rule_id: null } as const;

/** `changed` 收到评分变化（撤销、取消赞）后的值（1 赞、-1 踩、0 没有），供阅读器改回按钮状态。 */
export type FocusTrainingHandle = { rate: (article: ArticleView, up: boolean, changed?: (feedback: number) => void) => Promise<boolean> };
type Training = { article: ArticleView; features: FeatureView[]; candidates?: RuleCandidateView[]; dislike: boolean };

export const FocusTraining = forwardRef<FocusTrainingHandle, {
  session: FocusSession; open: boolean; style: CSSProperties;
  /** 同一篇的评分写入方共用，连续点赞 / 点踩按点击顺序落库。 */
  ratings: ArticleWriteOrder;
  /** 评分落库或撤销后通知各列表改卡片上的标记。 */
  onFeedback: (id: number, feedback: number) => void;
  onOpen: () => void; onClose: () => void; notify: (notice: FocusNotice) => void;
}>(function FocusTraining({ session, open, style, ratings, onFeedback, onOpen, onClose, notify }, ref) {
  const { t } = useTranslation("focus");
  const [training, setTraining] = useState<Training | null>(null);
  const confirming = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useImperativeHandle(ref, () => {
    const openRules = async (article: ArticleView, up: boolean) => {
      let candidates: RuleCandidateView[] | undefined;
      let features: FeatureView[] = [];
      try {
        // 踩的候选带近 30 天影响面；赞只加分，沿用原来的特征候选。
        if (up) features = await api.training(article.id);
        else candidates = await api.ruleCandidates(article.id);
      } catch (err) { notify({ message: formatNotice(t("dislike.rulesFailed"), err) }); return; }
      if (!(up ? features : candidates ?? []).length) { notify({ message: t("dislike.rulesEmpty"), holdMs: 2500 }); return; }
      setError("");
      setTraining({ article, features, candidates, dislike: !up });
      onOpen();
    };
    const changeTo = (article: ArticleView, feedback: number, changed?: (feedback: number) => void) => {
      onFeedback(article.id, feedback);
      changed?.(feedback);
    };
    // 不弹训练条：只出一条可撤销的提示；推荐日志的赞 / 踩等窗口结束才写，撤销了就不写。
    const quietNotice = (article: ArticleView, up: boolean, undo: RatingUndoView, changed?: (feedback: number) => void) => {
      let settled = false;
      const settle = () => { if (!settled) { settled = true; clearTimeout(timer); trackEvent(article.id, up ? "up" : "down"); } };
      const timer = setTimeout(settle, QUIET_RATING_MS);
      notify({
        message: t(up ? "like.recorded" : "dislike.recorded"), holdMs: QUIET_RATING_MS,
        actionLabel: t("dislike.undo"),
        onAction: () => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          void (async () => {
            try { await ratings.write(article.id, () => api.undoRating(article.id, undo)); }
            catch (err) { notify({ message: formatNotice(t("session.undoFailed"), err) }); return; }
            changeTo(article, undo.previous, changed);
            notify({ message: t(up ? "like.undone" : "dislike.undone"), holdMs: 2500 });
            // 踩会把文章移出列表，撤销后重读放回；赞不动列表。
            if (!up) await session.reload();
          })();
        },
        secondaryLabel: t("dislike.addRule"),
        onSecondary: () => { settle(); void openRules(article, up); },
      });
    };
    return {
      async rate(article, up, changed) {
        // 推荐详情打开时，抽查卡的赞踩仍走原有的揭示流程。
        const reveal = article.spot_check && session.getRanking().show_details;
        if (up && article.feedback === 1 && !reveal) {
          // 已赞再点：取消赞，恢复成没有评分。
          try { await ratings.write(article.id, () => api.undoRating(article.id, { up: true, ...NO_PREVIOUS })); }
          catch (err) { notify({ message: formatNotice(t("training.rateFailed"), err) }); return false; }
          changeTo(article, 0, changed);
          notify({ message: t("like.removed"), holdMs: 2500 });
          return false;
        }
        let undo: RatingUndoView;
        try {
          undo = await ratings.write(article.id, () => api.rate(article.id, up));
          if (reveal) trackEvent(article.id, up ? "up" : "down");
        } catch (err) { notify({ message: formatNotice(t("training.rateFailed"), err) }); return false; }
        onFeedback(article.id, up ? 1 : -1);
        if (article.spot_check) session.revealSpotCheck(article.id, up);
        if (reveal) {
          // 说明在卡片下方展开，这里只确认记下了。
          notify({ message: t("training.recorded"), holdMs: 2500 });
          return true;
        }
        quietNotice(article, up, undo, changed);
        // 不等重读：阅读器先标成已踩，提示里的撤销随后才能把它改回。
        if (!up) void session.reload();
        return true;
      },
    };
  }, [notify, onFeedback, onOpen, ratings, session, t]);
  if (!open || !training) return null;
  return <TrainingBar item={training} style={style} busy={busy} error={error}
    onClose={() => { if (!confirming.current) onClose(); }} onConfirm={async (feature, feedScope) => {
      if (confirming.current) return;
      confirming.current = true;
      setBusy(true);
      setError("");
      try {
        await api.confirmFeature({ articleId: training.article.id, feedId: training.article.feed_id, dislike: training.dislike, feedScope,
          dimension: feature.dimension, pattern: feature.pattern });
        onClose();
        await session.reload();
      } catch (err) {
        setError(formatNotice(t("training.saveFailed"), err));
      } finally {
        confirming.current = false;
        setBusy(false);
      }
    }} />;
});
