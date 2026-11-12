import { Icon } from "../../components/Icon";
import type { TranslationSessionView } from "./translationSession";
import { unitFailed } from "./translationState";

export function translationAction(view: TranslationSessionView): { kind: "recover" | "save" | "retry" | "start" | "regenerate"; label: string } {
  if (view.disconnected) return { kind: "recover", label: "恢复译文" };
  if (view.failure === "retry_unavailable") return { kind: "regenerate", label: "重新翻译全文" };
  const snapshot = view.data?.snapshot;
  if (snapshot?.save === "save_failed") return { kind: "save", label: "重试保存" };
  if (snapshot?.phase === "partial") return { kind: "retry", label: "重试未译部分" };
  // 零成功没有可复用的块；普通启动同样只补失败处，内存记录过期后也不会被迫走「重新翻译全文」。
  if (snapshot?.phase === "failed") return { kind: "start", label: "重试翻译" };
  if (snapshot?.phase === "invalidated") return { kind: "start", label: "重新翻译" };
  if (snapshot?.phase === "completed") return { kind: "regenerate", label: "重新翻译全文" };
  return { kind: "start", label: "翻译" };
}

const errors = {
  timeout: "部分内容翻译超时", network: "翻译服务连接失败", invalid_output: "部分译文格式无效",
  authentication: "翻译服务认证失败，请检查密钥", quota: "翻译服务额度不足，请检查账户", rate_limited: "翻译服务限流，请稍后重试",
  configuration: "翻译配置不可用，请检查模型和服务设置", invalidated: "正文或翻译配置已更新，请重新翻译",
  storage: "译文未保存，可继续阅读", capacity: "翻译任务已满，请等待已有任务结束", retry_unavailable: "未译部分记录已失效，请重新翻译全文",
};
export function TranslationStatus({ view, debug, onAction, onLocate }: {
  view: TranslationSessionView; debug: boolean; onAction: () => void; onLocate: () => void;
}) {
  const snapshot = view.data?.snapshot;
  const failed = snapshot?.units.filter(unitFailed).length ?? 0;
  const done = snapshot?.units.filter(unit => unit === "done").length ?? 0;
  const total = snapshot?.units.length ?? 0;
  const exception = view.issue || view.disconnected || snapshot?.save === "save_failed" || ["partial", "failed", "invalidated"].includes(snapshot?.phase ?? "");
  let message = view.note;
  if (!message && exception) message = snapshot?.save === "kept"
    ? `${snapshot.error ? `${errors[snapshot.error]}；` : ""}本次未全部译完，原有完整译文已保留`
    : snapshot?.error ? errors[snapshot.error] : "部分内容未译出，已保留原文";
  if (!message && debug && snapshot) message = view.busy ? snapshot.phase === "queued" ? "等待翻译" : "翻译中" : "翻译完成";
  if (!message) return null;
  return <div className={`reader-translation-status${exception || view.note ? " warning" : ""}`} role="status">
    <div className="reader-translation-copy"><Icon name={view.busy && !exception ? "spinner" : "info"} className={view.busy && !exception ? "spin" : ""} size={15} />
      <span>{message}</span>{debug && snapshot ? <small>已译 {done}/{total} 处{failed ? ` · ${failed} 处未译出` : ""}</small> : null}</div>
    {exception ? <div className="reader-translation-actions">
      {failed && view.data?.ready ? <button type="button" className="tool-btn" onClick={onLocate}>查看未译处</button> : null}
      <button type="button" className="tool-btn primary" onClick={onAction} disabled={view.busy}>{view.busy ? "处理中…" : translationAction(view).label}</button>
    </div> : null}
    {debug && total > 0 && !exception ? <progress aria-label="翻译进度" max={total} value={done} /> : null}
  </div>;
}
