import { forwardRef, useImperativeHandle, useRef, useState, type CSSProperties } from "react";
import { TrainingBar } from "../../components/TrainingBar";
import { api, type ArticleView, type FeatureView } from "../../lib/api";
import { trackEvent } from "../../lib/signals";
import { formatNotice } from "../../lib/notice";
import type { ArticleWriteOrder } from "../../lib/articleWriteOrder";
import type { FocusNotice, FocusSession } from "./focusSession";

export type FocusTrainingHandle = { rate: (article: ArticleView, up: boolean) => Promise<boolean> };
export const FocusTraining = forwardRef<FocusTrainingHandle, {
  session: FocusSession; open: boolean; style: CSSProperties;
  /** 同一篇的评分写入方共用，连续点赞 / 点踩按点击顺序落库。 */
  ratings: ArticleWriteOrder;
  onOpen: () => void; onClose: () => void; notify: (notice: FocusNotice) => void;
}>(function FocusTraining({ session, open, style, ratings, onOpen, onClose, notify }, ref) {
  const [training, setTraining] = useState<{ article: ArticleView; features: FeatureView[]; dislike: boolean } | null>(null);
  const confirming = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useImperativeHandle(ref, () => ({
    async rate(article, up) {
      try { await ratings.write(article.id, () => api.rate(article.id, up)); trackEvent(article.id, up ? "up" : "down"); }
      catch (err) { notify({ message: formatNotice("评分失败：", err) }); return false; }
      if (article.spot_check) session.revealSpotCheck(article.id, up);
      // 推荐详情关时抽查卡与普通卡片无异，照常进入训练条；说明留到打开详情时再显示。
      if (article.spot_check && session.getRanking().show_details) {
        notify({ message: "已记录偏好", holdMs: 2500 });
        return true;
      }
      const features = await api.training(article.id).catch(() => []);
      if (features.length) {
        setError("");
        setTraining({ article, features, dislike: !up });
        onOpen();
      } else {
        notify({ message: "已记录偏好", holdMs: 2500 });
        await session.reload();
      }
      return true;
    },
  }), [notify, onOpen, ratings, session]);
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
        setError(formatNotice("规则保存失败：", err));
      } finally {
        confirming.current = false;
        setBusy(false);
      }
    }} />;
});
