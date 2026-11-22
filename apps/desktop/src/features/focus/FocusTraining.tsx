import { forwardRef, useImperativeHandle, useRef, useState, type CSSProperties } from "react";
import { TrainingBar } from "../../components/TrainingBar";
import { api, type ArticleView, type FeatureView, type RatingUndoView, type RuleCandidateView } from "../../lib/api";
import { useTranslation } from "../../i18n";
import { trackEvent } from "../../lib/signals";
import { formatNotice } from "../../lib/notice";
import type { ArticleWriteOrder } from "../../lib/articleWriteOrder";
import type { FocusNotice, FocusSession } from "./focusSession";

/** 阅读器里点踩后的提示时长，也是撤销窗口（阶段 11 · 踩的反馈）。 */
const READER_DISLIKE_MS = 6000;

/** `undone` 收到撤销后恢复的评分（1 赞、-1 踩、0 没有），供阅读器改回按钮状态。 */
export type FocusTrainingHandle = { rate: (article: ArticleView, up: boolean, undone?: (feedback: number) => void) => Promise<boolean> };
type Training = { article: ArticleView; features: FeatureView[]; candidates?: RuleCandidateView[]; dislike: boolean };

export const FocusTraining = forwardRef<FocusTrainingHandle, {
  session: FocusSession; open: boolean; style: CSSProperties;
  /** 同一篇的评分写入方共用，连续点赞 / 点踩按点击顺序落库。 */
  ratings: ArticleWriteOrder;
  onOpen: () => void; onClose: () => void; notify: (notice: FocusNotice) => void;
}>(function FocusTraining({ session, open, style, ratings, onOpen, onClose, notify }, ref) {
  const { t } = useTranslation("focus");
  const [training, setTraining] = useState<Training | null>(null);
  const confirming = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useImperativeHandle(ref, () => {
    const openRules = async (article: ArticleView) => {
      let candidates: RuleCandidateView[];
      try { candidates = await api.ruleCandidates(article.id); }
      catch (err) { notify({ message: formatNotice(t("dislike.rulesFailed"), err) }); return; }
      if (!candidates.length) { notify({ message: t("dislike.rulesEmpty"), holdMs: 2500 }); return; }
      setError("");
      setTraining({ article, features: [], candidates, dislike: true });
      onOpen();
    };
    // 不弹训练条：只出一条可撤销的提示；推荐日志的「踩」等窗口结束才写，撤销了就不写。
    const dislikeNotice = (article: ArticleView, undo: RatingUndoView, undone?: (feedback: number) => void) => {
      let settled = false;
      const settle = () => { if (!settled) { settled = true; clearTimeout(timer); trackEvent(article.id, "down"); } };
      const timer = setTimeout(settle, READER_DISLIKE_MS);
      notify({
        message: t("dislike.recorded"), holdMs: READER_DISLIKE_MS,
        actionLabel: t("dislike.undo"),
        onAction: () => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          void (async () => {
            try { await ratings.write(article.id, () => api.undoRating(article.id, undo)); }
            catch (err) { notify({ message: formatNotice(t("session.undoFailed"), err) }); return; }
            undone?.(undo.previous);
            notify({ message: t("dislike.undone"), holdMs: 2500 });
            await session.reload();
          })();
        },
        secondaryLabel: t("dislike.addRule"),
        onSecondary: () => { settle(); void openRules(article); },
      });
    };
    return {
      async rate(article, up, undone) {
        // 推荐详情打开时，抽查卡的踩仍走原有的揭示流程。
        const quiet = !up && !(article.spot_check && session.getRanking().show_details);
        let undo: RatingUndoView;
        try {
          undo = await ratings.write(article.id, () => api.rate(article.id, up));
          if (!quiet) trackEvent(article.id, up ? "up" : "down");
        } catch (err) { notify({ message: formatNotice(t("training.rateFailed"), err) }); return false; }
        if (article.spot_check) session.revealSpotCheck(article.id, up);
        if (quiet) {
          dislikeNotice(article, undo, undone);
          // 不等重读：阅读器先标成已踩，提示里的撤销随后才能把它改回。
          void session.reload();
          return true;
        }
        // 推荐详情关时抽查卡与普通卡片无异，照常进入训练条；说明留到打开详情时再显示。
        if (article.spot_check && session.getRanking().show_details) {
          notify({ message: t("training.recorded"), holdMs: 2500 });
          return true;
        }
        const features = await api.training(article.id).catch(() => []);
        if (features.length) {
          setError("");
          setTraining({ article, features, dislike: !up });
          onOpen();
        } else {
          notify({ message: t("training.recorded"), holdMs: 2500 });
          await session.reload();
        }
        return true;
      },
    };
  }, [notify, onOpen, ratings, session, t]);
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
