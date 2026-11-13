import { useEffect, useId, useState, useSyncExternalStore } from "react";
import { api, type ChatConfirmEvent } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { Trans, useTranslation } from "../../i18n";

/** 一轮对话里的过程信息：一行进度、需要看见的提示、等待回应的确认。只存最新一份，不按线程保留历史。 */
export type ChatActivityState = {
  line: string;
  notice: { text: string; version: string } | null;
  confirm: ChatConfirmEvent | null;
};
const EMPTY: ChatActivityState = { line: "", notice: null, confirm: null };

type AnswerListener = (confirm: ChatConfirmEvent, ok: boolean) => void;

export function createActivityStore() {
  let state = EMPTY;
  const listeners = new Set<() => void>();
  const answers = new Set<AnswerListener>();
  return {
    get: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    set(patch: Partial<ChatActivityState>) {
      state = { ...state, ...patch };
      for (const listener of [...listeners]) listener();
    },
    /** 用户回应了确认（宿主已收到）；等待过程据此记下「已允许 / 已拒绝」。 */
    onAnswer(listener: AnswerListener) {
      answers.add(listener);
      return () => { answers.delete(listener); };
    },
    answered(confirm: ChatConfirmEvent, ok: boolean) {
      for (const listener of [...answers]) listener(confirm, ok);
    },
  };
}
export type ActivityStore = ReturnType<typeof createActivityStore>;

/** 输入框上方的过程条：确认条优先，其次需要看见的提示，最后是一行进度。 */
export function ChatActivity({ store }: { store: ActivityStore }) {
  const { t } = useTranslation("chat");
  const { line, notice, confirm } = useSyncExternalStore(store.subscribe, store.get, store.get);

  // 提示真正显示出来才算看见；同一版本宿主只会发一次。
  const version = notice?.version ?? "";
  useEffect(() => {
    if (version) void api.acknowledgePiVersionNotice(version).catch(() => {});
  }, [version]);

  if (!line && !notice && !confirm) return null;
  return (
    <div className="chat-activity">
      {confirm && <ConfirmCard key={confirm.id} confirm={confirm} store={store} />}
      {notice && (
        <div className="chat-notice" role="status">
          <span>{notice.text}</span>
          <button type="button" className="btn ghost sm" onClick={() => store.set({ notice: null })}>{t("activity.dismiss")}</button>
        </div>
      )}
      {line && !confirm && <p className="chat-status-line" role="status">{line}</p>}
    </div>
  );
}

/**
 * 确认卡：读取网页等用宿主给的标题与说明；外部工具（带服务名）写「允许使用 服务 的 工具？」、参数，
 * 宿主允许时可勾「本次对话里不再问」（只随「允许」生效）。
 */
function ConfirmCard({ confirm, store }: { confirm: ChatConfirmEvent; store: ActivityStore }) {
  const { t } = useTranslation("chat");
  const [answering, setAnswering] = useState(false);
  const [remember, setRemember] = useState(false);
  const [error, setError] = useState("");
  const messageId = useId();
  const external = !!confirm.server;
  const answer = async (ok: boolean) => {
    if (answering) return;
    setAnswering(true);
    setError("");
    try {
      await api.confirmFetch(confirm.id, ok, ok && remember);
      store.answered(confirm, ok);
      if (store.get().confirm?.id === confirm.id) store.set({ confirm: null });
    } catch (err) {
      setError(formatNotice(t("activity.answerFailed"), err));
    } finally {
      setAnswering(false);
    }
  };
  return (
    <div className="chat-confirm" role="alertdialog" aria-label={external ? t("activity.allowUse", { server: confirm.server, tool: confirm.tool }) : confirm.title}
      aria-describedby={messageId}>
      <div>
        {external
          ? <strong><Trans t={t} i18nKey="activity.allowUseRich" values={{ server: confirm.server, tool: confirm.tool }} components={{ srv: <span className="chat-srv" /> }} /></strong>
          : <strong>{confirm.title}</strong>}
        <p id={messageId}>{external ? (confirm.args ? t("activity.mutatesWithArgs") : t("activity.mutates")) : confirm.message}</p>
        {external && confirm.args ? <pre className="chat-confirm-args">{confirm.args}</pre> : null}
        {external && confirm.can_remember ? (
          <label className="chat-confirm-remember">
            <input type="checkbox" checked={remember} disabled={answering} onChange={(e) => setRemember(e.target.checked)} />{t("activity.remember", { tool: confirm.tool })}
          </label>
        ) : null}
        {confirm.timeout_ms ? <small className="muted">{t("activity.timeout", { count: Math.round(confirm.timeout_ms / 60000) || 1 })}</small> : null}
        {error && <p className="form-status error" role="alert">{error}</p>}
      </div>
      <button type="button" className="btn primary sm" disabled={answering} onClick={() => void answer(true)}>{t("activity.allow")}</button>
      <button type="button" className="btn sm" disabled={answering} onClick={() => void answer(false)}>{t("activity.deny")}</button>
    </div>
  );
}
