import { Trans, useTranslation } from "../../i18n";
import { WebReadingBar } from "./WebReading";
import type { WebWarning } from "./useWebLinks";

export function WebSafetyWarning({ warning, onCancel, onContinue }: {
  warning: WebWarning; onCancel: () => void; onContinue: () => void;
}) {
  const { t } = useTranslation("reader");
  const danger = warning.safety.warnings.some(w => w.kind !== "NonHttps");
  return <>
      <WebReadingBar tone={danger ? "danger" : "warn"} page={{ url: warning.href }} onBack={onCancel} />
      <div className="drawer-body"><div className="reader-column web-column">
        <div className="web-tip web-warning" data-tone={danger ? "danger" : "warn"} role="alert">
          <div className="web-tip-text">{warning.safety.warnings.map((item, index) => <div key={index}>
            {item.kind === "MixedScript" ? <><strong>{t("webWarning.mixedScript.title")}</strong><p><Trans t={t} i18nKey="webWarning.mixedScript.detail" values={{ unicode: item.unicode, punycode: item.punycode }} components={{ code: <code /> }} /></p></>
              : item.kind === "HostMismatch" ? <><strong>{t("webWarning.hostMismatch.title")}</strong><p><Trans t={t} i18nKey="webWarning.hostMismatch.detail" values={{ text: item.text_host, actual: warning.safety.actual_host }} components={{ code: <code /> }} /></p></>
                : <><strong>{t("webWarning.nonHttps.title")}</strong><p>{t("webWarning.nonHttps.detail")}</p></>}
          </div>)}</div>
          <div className="web-tip-actions"><button type="button" className="btn primary" onClick={onCancel}>{t("webWarning.cancel")}</button><button type="button" className="btn ghost" onClick={onContinue}>{danger ? t("webWarning.openAnyway") : t("webWarning.continue")}</button></div>
          <small>{t("webWarning.note")}</small>
        </div>
      </div></div>
  </>;
}
