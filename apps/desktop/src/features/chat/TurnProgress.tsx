import { useEffect, useId, useState } from "react";
import { Icon } from "../../components/Icon";
import { i18n, Trans, useTranslation } from "../../i18n";
import { summarize, type LiveTurn, type Trace, type TraceStep } from "./turnTrace";

/** 刚发出时先等一下再出现，冷启动的「启动对话引擎」不会先闪一下「思考」。 */
const SHOW_AFTER_MS = 150;

function useNow(tickMs: number, running: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), tickMs);
    return () => window.clearInterval(timer);
  }, [tickMs, running]);
  return now;
}

const seconds = (ms: number) => (Math.max(0, ms) / 1000).toFixed(1);

type Shown = Pick<TraceStep, "kind" | "tool" | "server" | "serverTool">;

export function chatStepLabel(step: Shown): string {
  if (step.kind === "think") return i18n.t("progress.think", { ns: "chat" });
  if (step.kind === "engine") return i18n.t("steps.engine", { ns: "chat" });
  if (step.server) return i18n.t("steps.external", { ns: "chat", server: step.server, tool: step.serverTool });
  const key = `steps.tools.${step.tool}`;
  return i18n.exists(key, { ns: "chat" }) ? String(i18n.t(key as never, { ns: "chat" })) : i18n.t("steps.tool", { ns: "chat", tool: step.tool });
}

/** 当前这一步：还没结束的引擎 / 工具；都结束了就是「思考」（键随上一步变，换字时上推一次）。 */
function currentStep(turn: LiveTurn): Shown & { key: string; detail: string } {
  const open = turn.steps.filter((step) => step.endedAt === null).at(-1);
  if (open) return { key: open.id, kind: open.kind, tool: open.tool, detail: open.detail, server: open.server, serverTool: open.serverTool };
  const last = turn.steps.at(-1);
  return { key: `think-${last?.id ?? "start"}`, kind: "think", tool: "", detail: "", server: "", serverTool: "" };
}

/** 外部工具写「使用 服务 的 工具」，服务名等宽；其余按步骤与工具 id 取文案。 */
function StepText({ step }: { step: Shown }) {
  const { t } = useTranslation("chat");
  if (!step.server) return <>{chatStepLabel(step)}</>;
  return <Trans t={t} i18nKey="progress.useTool" values={{ server: step.server, tool: step.serverTool }} components={{ srv: <span className="chat-srv" /> }} />;
}

/**
 * 等待过程（单行）：脉冲点 + 当前步骤（扫光文字）+ 参数摘要 + 计时。步骤切换时新文字自下方上推出现，
 * 旧文字直接换掉。开始出字后由 `TraceRow` 接替；出字后又调用工具时，回复下方再显示一行，从 `since` 起计时。
 */
export function WaitingLine({ turn, since = turn.startedAt }: { turn: LiveTurn; since?: number }) {
  const { t } = useTranslation("chat");
  const now = useNow(250, true);
  const visible = turn.steps.length > 0 || now - turn.startedAt >= SHOW_AFTER_MS;
  if (!visible) return null;
  const step = currentStep(turn);
  const asking = turn.asking;
  return (
    <p className="chat-wait" role="status" data-paused={asking ? "" : undefined}>
      <span className="chat-wait-pulse" aria-hidden="true" />
      {/* 等用户确认外部工具时停下脉冲与扫光，写清在等什么。 */}
      {asking ? (
        <span className="chat-wait-step" key={`ask-${asking.server}-${asking.tool}`}>
          <span className="chat-wait-asking"><Trans t={t} i18nKey="progress.asking" values={{ server: asking.server, tool: asking.tool }} components={{ srv: <span className="chat-srv" /> }} /></span>
        </span>
      ) : (
        <span className="chat-wait-step" key={step.key}>
          <span className="chat-wait-label"><StepText step={step} /></span>
          {step.detail ? <span className="chat-wait-detail">{step.detail}</span> : null}
        </span>
      )}
      <span className="chat-wait-time" aria-hidden="true">· {t("progress.seconds", { n: Math.floor((now - since) / 1000) })}</span>
    </p>
  );
}

function StepRow({ step }: { step: TraceStep }) {
  const { t } = useTranslation("chat");
  return (
    <li data-error={step.error ? "" : undefined}>
      <span className="chat-trace-icon" aria-hidden="true"><Icon name={step.error ? "info" : "check"} size={12} /></span>
      <span className="chat-trace-label">{step.server ? <><span className="chat-srv">{step.server}</span> · {step.serverTool}</> : chatStepLabel(step)}</span>
      {step.approval ? <span className="chat-trace-approval" data-denied={step.approval === "denied" ? "" : undefined}>
        · {step.approval === "allowed" ? t("progress.allowed") : t("progress.denied")}</span> : null}
      {step.detail ? <span className="chat-trace-detail">{step.detail}</span> : null}
      <span className="chat-trace-time">{seconds((step.endedAt ?? step.startedAt) - step.startedAt)}s</span>
    </li>
  );
}

/** 「查阅了 N 处 · 用时 X 秒 ⌄」：开始出字时由等待过程收成这一行，点开列出各步骤、参数摘要与耗时。 */
export function TraceRow({ trace, enter = false }: { trace: Trace; enter?: boolean }) {
  const { t } = useTranslation("chat");
  const [open, setOpen] = useState(false);
  const listId = useId();
  const summary = [trace.tools > 0 && t("progress.tools", { count: trace.tools }), trace.external > 0 && t("progress.external", { count: trace.external }), t("progress.elapsed", { seconds: seconds(trace.elapsed) })]
    .filter(Boolean).join(" · ");
  return (
    <div className="chat-trace" data-enter={enter ? "" : undefined}>
      <button type="button" className="chat-trace-toggle" aria-expanded={open} aria-controls={listId} onClick={() => setOpen((value) => !value)}>
        {summary}<Icon name="chevronDown" size={12} />
      </button>
      {open ? <ul className="chat-trace-list" id={listId}>{trace.steps.map((step) => <StepRow key={step.id} step={step} />)}</ul> : null}
    </div>
  );
}

/** 进行中一轮的摘要：开始出字后到落库前，挂在流式回复上方；算到最近一次变化，出字后的工具调用结束时更新。 */
export function liveTrace(turn: LiveTurn): Trace | null {
  return turn.answeringAt === null ? null : summarize(turn, turn.at);
}
