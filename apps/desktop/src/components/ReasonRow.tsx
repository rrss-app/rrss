import type { ReasonView } from "../lib/api";
import { useTranslation } from "../i18n";

export function ReasonRow({ reasons }: { reasons: ReasonView[] }) {
  const { t } = useTranslation("focus");
  if (!reasons.length) return null;
  return <p className="reader-reasons">{t("reasonRow.because")}{" "}{reasons.map((reason, index) => <span key={`${reason.rule_id}-${reason.label}`}>
    {index ? " · " : ""}<strong>{reason.label}</strong>{reason.weight ? <span className={reason.weight > 0 ? "up" : "down"}> {reason.weight > 0 ? "+" : "−"}{Math.abs(reason.weight)}</span> : null}
  </span>)}</p>;
}
