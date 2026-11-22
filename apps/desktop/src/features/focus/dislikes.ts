import { api, type ArticleView, type RatingUndoView } from "../../lib/api";
import { DISLIKE_HOLD_MS, type DislikeAction, type DislikeState } from "../../components/DislikeVeil";
import { i18n } from "../../i18n";
import { formatNotice } from "../../lib/notice";
import { trackEvent } from "../../lib/signals";
import type { FocusNotice } from "./focusSession";

/** 卡片淡出的时长，与 `.article[data-dismissed="leaving"]` 的过渡一致。 */
const LEAVE_MS = 200;

/** 同一篇的评分写入共用的顺序（见 `ArticleWriteOrder`）。 */
export type RatingWrite = <T>(request: () => Promise<T>) => Promise<T>;

type Entry = { row: ArticleView; write: RatingWrite; rated: Promise<RatingUndoView | null>; timer?: ReturnType<typeof setTimeout> };

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * 焦点卡片的点踩窗口：评分点下即写；窗口结束才写推荐日志里的「踩」并把卡片换成下一条候选，
 * 撤销则恢复原评分、不写日志。列表重读或会话停止时，未结束的窗口直接生效。
 */
export function createDislikes(host: {
  read: () => Record<number, DislikeState>;
  write: (next: Record<number, DislikeState>) => void;
  /** 取补位：当前排序下列表之外的下一条或一组；取不到返回空。 */
  replacement: (row: ArticleView) => Promise<ArticleView[]>;
  /** 把这一行换成补位（可为空）；之后这篇不再出现在当前列表。 */
  replace: (row: ArticleView, rows: ArticleView[]) => void;
  /** 列表重读时直接生效：只记下不再显示这篇。 */
  dismiss: (id: number) => void;
  rulesChanged: () => void;
  notify: (notice: FocusNotice) => void;
}) {
  const entries = new Map<number, Entry>();
  const set = (id: number, patch: Partial<DislikeState> | null) => {
    const all = { ...host.read() };
    if (patch === null) delete all[id];
    else all[id] = { ...all[id], ...patch };
    host.write(all);
  };
  const arm = (id: number, ms: number, restart = false) => {
    const entry = entries.get(id);
    if (!entry) return;
    clearTimeout(entry.timer);
    entry.timer = setTimeout(() => void finish(id), ms);
    const run = (host.read()[id]?.run ?? 0) + (restart ? 1 : 0);
    set(id, { deadline: Date.now() + ms, remainingMs: ms, run });
  };
  const pause = (id: number) => {
    const entry = entries.get(id);
    const current = host.read()[id];
    if (!entry || !current || current.deadline === null) return;
    clearTimeout(entry.timer);
    entry.timer = undefined;
    set(id, { deadline: null, remainingMs: Math.max(0, current.deadline - Date.now()) });
  };
  const settle = (id: number) => {
    const entry = entries.get(id);
    if (!entry) return null;
    clearTimeout(entry.timer);
    entries.delete(id);
    trackEvent(id, "down");
    return entry;
  };

  async function finish(id: number) {
    const entry = entries.get(id);
    if (!entry || host.read()[id]?.leaving) return;
    clearTimeout(entry.timer);
    set(id, { leaving: true, deadline: null, rules: null });
    const [rows] = await Promise.all([host.replacement(entry.row).catch(() => []), wait(LEAVE_MS)]);
    // 期间列表重读过：已经直接生效，不再替换。
    if (entries.get(id) !== entry) return;
    settle(id);
    set(id, null);
    host.replace(entry.row, rows);
  }

  return {
    /** 点踩：卡片先虚化，评分失败再恢复并提示。 */
    async start(row: ArticleView, write: RatingWrite): Promise<boolean> {
      if (entries.has(row.id)) return true;
      const rated = write(() => api.rate(row.id, false)).then((undo) => undo, () => null);
      const entry: Entry = { row, write, rated };
      entries.set(row.id, entry);
      set(row.id, { deadline: null, remainingMs: DISLIKE_HOLD_MS, run: 0, leaving: false, rules: null });
      arm(row.id, DISLIKE_HOLD_MS);
      if ((await rated) !== null) return true;
      if (entries.get(row.id) === entry) {
        clearTimeout(entry.timer);
        entries.delete(row.id);
        set(row.id, null);
      }
      host.notify({ message: i18n.t("training.rateFailed", { ns: "focus" }) });
      return false;
    },
    async act(id: number, action: DislikeAction) {
      const entry = entries.get(id);
      const current = host.read()[id];
      if (!entry || !current || current.leaving) return;
      if (action.kind === "hold") {
        if (current.rules) return;
        if (action.held) pause(id);
        else if (current.deadline === null) arm(id, Math.max(current.remainingMs, 1));
        return;
      }
      if (action.kind === "rules") {
        if (!action.open) { set(id, { rules: null }); arm(id, DISLIKE_HOLD_MS, true); return; }
        if (current.rules) return;
        pause(id);
        set(id, { rules: { loading: true, candidates: [], error: "", busy: false } });
        try {
          const candidates = await api.ruleCandidates(id);
          if (entries.get(id) === entry && host.read()[id]?.rules) set(id, { rules: { loading: false, candidates, error: "", busy: false } });
        } catch (err) {
          if (entries.get(id) === entry && host.read()[id]?.rules) {
            set(id, { rules: { loading: false, candidates: [], error: formatNotice(i18n.t("dislike.rulesFailed", { ns: "focus" }), err), busy: false } });
          }
        }
        return;
      }
      if (action.kind === "rule") {
        const rules = current.rules;
        if (!rules || rules.busy) return;
        set(id, { rules: { ...rules, busy: true, error: "" } });
        try {
          await api.confirmFeature({ articleId: id, feedId: entry.row.feed_id, dislike: true, feedScope: action.feedScope,
            dimension: action.candidate.dimension, pattern: action.candidate.pattern });
        } catch (err) {
          const latest = host.read()[id]?.rules;
          if (entries.get(id) === entry && latest) set(id, { rules: { ...latest, busy: false, error: formatNotice(i18n.t("training.saveFailed", { ns: "focus" }), err) } });
          return;
        }
        host.rulesChanged();
        if (entries.get(id) === entry) set(id, { rules: null });
        void finish(id);
        return;
      }
      clearTimeout(entry.timer);
      entries.delete(id);
      set(id, null);
      const undo = await entry.rated;
      if (!undo) return;
      try {
        await entry.write(() => api.undoRating(id, undo));
        host.notify({ message: i18n.t("dislike.undone", { ns: "focus" }), holdMs: 2500 });
      } catch (err) {
        host.notify({ message: formatNotice(i18n.t("session.undoFailed", { ns: "focus" }), err) });
      }
    },
    /** 列表重读、换范围或会话停止：未结束的窗口直接生效。 */
    commitAll() {
      if (!entries.size) return;
      for (const id of [...entries.keys()]) if (settle(id)) host.dismiss(id);
      host.write({});
    },
  };
}
