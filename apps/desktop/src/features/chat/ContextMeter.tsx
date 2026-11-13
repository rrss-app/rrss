import { useEffect, useRef, useState } from "react";
import type { ContextUsageView, ModelChoice } from "../../lib/api";
import { usePresence } from "../../lib/motion";
import { useTranslation } from "../../i18n";
import { chatDefault, findEntry, modelName, usePoolChoices } from "../../settings/modelChoices";

/** 上限从哪来：设置里填的、引擎给的默认值、上次发送时记下的。 */
export type WindowSource = "user" | "engine" | "last";

const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1).replace(/\.0$/, "")}K` : String(n));

/** 用量比例；窗口未知或不大于零时没有比例，不当作零或无限。 */
export function contextPercent(used: number | undefined, window: number | null) {
  return used === undefined || !window || window <= 0 ? null : Math.min(100, Math.round((used / window) * 100));
}
const toneOf = (pct: number | null) => (pct === null ? undefined : pct >= 95 ? "danger" : pct >= 80 ? "warn" : undefined);

/**
 * 输入区的上下文指示：只画圆环，点开看用量、上限与来源。用量是上一轮结束（或压缩后）引擎报告的当前占用，
 * 80% 起转警示色、95% 起转危险色。
 */
export function ContextMeter({ usage, limit, source, model }: {
  usage: ContextUsageView | null;
  /** 上下文上限（token）；未知为 null。 */
  limit: number | null;
  source: WindowSource | null;
  model: string;
}) {
  const { t } = useTranslation("chat");
  const [open, setOpen] = useState(false);
  // 详情卡从圆环一角展开，收起留 100ms 退场。
  const presence = usePresence(open, 100);
  const root = useRef<HTMLSpanElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const pct = contextPercent(usage?.used, limit);
  const tone = toneOf(pct);

  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const escape = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing) return;
      e.preventDefault(); e.stopImmediatePropagation(); setOpen(false); trigger.current?.focus();
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", escape, true);
    return () => { document.removeEventListener("pointerdown", outside); window.removeEventListener("keydown", escape, true); };
  }, [open]);

  const r = 6.5;
  const c = 2 * Math.PI * r;
  const label = !usage ? t("meter.usageNone") : pct === null ? t("meter.usageTokens", { used: k(usage.used) }) : t("meter.usagePercent", { pct });
  const limitText = !limit ? t("meter.limitUnknown")
    : source === "user" ? t("meter.limitUser", { limit: k(limit), model })
    : source === "engine" ? t("meter.limitEngine", { limit: k(limit), model })
    : t("meter.limitLast", { limit: k(limit) });
  const heading = !usage ? t("meter.headingNone")
    : limit ? (pct === null ? t("meter.headingLimit", { used: k(usage.used), limit: k(limit) }) : t("meter.headingLimitPercent", { used: k(usage.used), limit: k(limit), pct }))
    : (pct === null ? t("meter.headingUsed", { used: k(usage.used) }) : t("meter.headingUsedPercent", { used: k(usage.used), pct }));
  return (
    <span className="context-meter" ref={root}>
      <button type="button" ref={trigger} className="context-meter-ring" data-tone={tone} aria-label={t("meter.ariaLabel", { label })}
        aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <circle className="track" cx="8" cy="8" r={r} />
          {pct ? <circle className="fill" cx="8" cy="8" r={r} strokeDasharray={`${(c * pct) / 100} ${c}`} /> : null}
        </svg>
      </button>
      {presence.mounted && (
        <div className="context-meter-tip" role="dialog" aria-label={t("meter.dialog")} data-tone={tone}
          data-leaving={presence.leaving ? "" : undefined} inert={presence.leaving} aria-hidden={presence.leaving || undefined}>
          <strong>{heading}</strong>
          {pct !== null && <div className="context-meter-bar"><i style={{ width: `${pct}%` }} /></div>}
          <p>{usage ? t("meter.noteUsage") : t("meter.noteNoRecord")}{limitText}</p>
          {pct !== null && pct >= 80 && (
            <p>{pct >= 95 ? t("meter.warnNear") : t("meter.warnSoon")}</p>
          )}
        </div>
      )}
    </span>
  );
}

/** 按这条对话当前选的模型定上限：设置里填的优先，其次引擎握手报告的默认值，都没有用上次发送时记下的。 */
export function ChatContextMeter({ value, usage }: { value: ModelChoice | null; usage: ContextUsageView | null }) {
  const { t } = useTranslation("chat");
  const { pool, bindings } = usePoolChoices();
  const current = value ?? (bindings?.chat_uses_own_config ? null : chatDefault(bindings));
  const entry = findEntry(pool, current);
  const [limit, source]: [number | null, WindowSource | null] = entry?.context_window ? [entry.context_window, "user"]
    : entry?.engine_context_window ? [entry.engine_context_window, "engine"]
    : usage?.window ? [usage.window, "last"] : [null, null];
  return <ContextMeter usage={usage} limit={limit} source={source} model={entry ? modelName(entry) : current?.model ?? t("meter.thisModel")} />;
}
