import type { ChatErrorEvent } from "../../lib/api";
import { Icon } from "../../components/Icon";
import type { ChatFailure } from "./conversationStore";
import { TraceRow } from "./TurnProgress";

export type ChatFixTab = NonNullable<ChatFailure["fix"]>;

/** 上游原文可能是整页 HTML 或很长的 JSON：界面只留开头，完整内容在日志里。 */
const DETAIL_MAX = 600;

/** 错误类别的界面文案（阶段 18 错误分类）；`fix` 指向能修好它的设置页。 */
export function describeChatError(event: ChatErrorEvent): ChatFailure {
  const message = event.message.trim();
  switch (event.kind) {
    case "not_installed": return { title: "未找到对话引擎", detail: "可以在设置里下载。", fix: "assistant" };
    case "version": return { title: "对话引擎版本过低", detail: "请在设置里升级。", fix: "assistant" };
    case "config": return { title: "对话模型不可用", detail: message || "还没有可用的对话模型。", fix: "model" };
    case "model": return { title: "模型服务出错", detail: message || "没有返回原因。" };
    // 已收到的内容照常留在上方，这里不再重复。
    case "process_exit": return { title: "对话引擎意外退出", detail: "下次发送会自动重启。" };
    default: return { title: message || "对话出错", detail: "" };
  }
}

/** 没有会话位置可挂时（线程未知）退回一句话的通知。 */
export function failureText(failure: ChatFailure) {
  return failure.detail ? `${failure.title}：${failure.detail}` : failure.title;
}

function clip(text: string) {
  return text.length > DETAIL_MAX ? `${text.slice(0, DETAIL_MAX - 1)}…` : text;
}

/**
 * 回复位置的失败提示：标题说哪一环出错，下面是脱敏后的原因；能在设置里修好的带「去设置」。
 * 这一轮没有回复时，上方保留做过的步骤（哪个工具出错一眼可见）。
 */
export function FailureNotice({ failure, onFix }: { failure: ChatFailure; onFix?: (tab: ChatFixTab) => void }) {
  const fix = failure.fix;
  return (
    <div className="chat-turn chat-failure-turn">
      {failure.trace ? <TraceRow trace={failure.trace} /> : null}
      <div className="notice error chat-failure" role="alert">
        <Icon name="warning" size={14} />
        <div className="chat-failure-body">
          <strong>{failure.title}</strong>
          {failure.detail ? <p>{clip(failure.detail)}</p> : null}
        </div>
        {fix && onFix ? <button type="button" className="btn sm" onClick={() => onFix(fix)}>去设置</button> : null}
      </div>
    </div>
  );
}
