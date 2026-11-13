import type { WebVerifyStateEvent } from "../../bindings";
import { locale, Trans, useTranslation } from "../../i18n";

/** 过盾进度：画在原网页顶栏下方、网页区域之外。不自动切换视图，也不关掉原网页。 */
export function WebVerifyStrip({ event, busy, onAbandon, onExtract, onRead }: {
  event: WebVerifyStateEvent; busy: boolean; onAbandon: () => void; onExtract: () => void; onRead: () => void;
}) {
  const { t } = useTranslation("reader");
  const ready = event.state === "Extracted", miss = event.state === "NotExtracted", wait = event.state === "WaitingVerification";
  const done = ready ? 3 : wait ? 1 : 2;
  return <div className="web-verify-strip" data-tone={ready ? "accent" : miss || wait ? "warn" : undefined} aria-label={t("verify.progress")}>
    <span className="web-verify-steps" aria-hidden="true">{[0, 1, 2].map(i => <i key={i} data-done={i < done || undefined} />)}</span>
    <span className="web-verify-text" role="status">{ready
      ? <Trans t={t} i18nKey="verify.extracted" values={{ title: event.title, chars: event.text_chars?.toLocaleString(locale()), images: event.image_count }} components={{ b: <b /> }} />
      : miss ? <><b>{t("verify.missed")}</b>{event.reason ? t("verify.missedReason", { reason: event.reason }) : ""}</>
      : wait ? <Trans t={t} i18nKey="verify.waiting" components={{ b: <b /> }} />
      : <Trans t={t} i18nKey="verify.extracting" components={{ b: <b /> }} />}</span>
    <span className="web-verify-actions">
      {ready ? <>
        <button type="button" className="btn sm primary" disabled={busy || event.article_id == null} onClick={onRead}>{t("verify.read")}</button>
        <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={onExtract}>{t("verify.retryLater")}</button>
      </> : miss ? <button type="button" className="btn sm primary" disabled={busy} aria-busy={busy} onClick={onExtract}>{t("verify.retryNow")}</button> : null}
      {!ready ? <button type="button" className="btn sm ghost" onClick={onAbandon}>{t("verify.abandon")}</button> : null}
    </span>
  </div>;
}
