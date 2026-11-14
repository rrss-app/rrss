import type { ReasonView } from "../lib/api";
import { useTranslation } from "../i18n";
import { formatReason, formatReasonNumber } from "../lib/reasons";

export function ReasonRow({ reasons }: { reasons: ReasonView[] }) {
  const { t } = useTranslation("focus");
  if (!reasons.length) return null;
  return <p className="reader-reasons">{t("reasonRow.because")}{" "}{reasons.map((reason, index) => <span key={`${reason.rule_id}-${index}`}>
    {index ? " · " : ""}<strong>{formatReason(reason)}</strong>{reason.weight ? <span className={reason.weight > 0 ? "up" : "down"}> {reason.weight > 0 ? "+" : "−"}{formatReasonNumber(Math.abs(reason.weight))}</span> : null}
  </span>)}</p>;
}
