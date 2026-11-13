import { useContext, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { settingsMutationOptions, settingsSchemaOptions } from "../../settings/settingsQuery";
import { useSettingsWrite } from "../../settings/editState";
import { WebLinksContext, webFailureReasons } from "../../lib/webLinks";
import type { FulltextError, OpenWebLinkView } from "../../bindings";
import { openExternal } from "../../lib/api";
import { RelativeTime } from "../../components/RelativeTime";
import { Icon } from "../../components/Icon";
import { locale, Trans, useTranslation } from "../../i18n";

export type WebPage = { url: string; info?: OpenWebLinkView; error?: FulltextError };
function webHost(page: WebPage) {
  return new URL(page.info?.actual_url ?? page.url).hostname;
}

type WebBarProps = {
  page: WebPage; returnTitle?: string; onBack: () => void; onCollect?: () => void; collecting?: boolean;
  tone?: "warn" | "danger"; view?: "reader" | "web"; onRead?: () => void;
};

export function WebReadingNav({ page, returnTitle, onBack, tone, live = false }: Pick<WebBarProps, "page" | "returnTitle" | "onBack" | "tone"> & { live?: boolean }) {
  const { t } = useTranslation("reader");
  const url = page.info?.actual_url ?? page.url;
  const insecure = new URL(url).protocol !== "https:";
  return <>
    <button type="button" className="web-tool web-back" aria-label={t("web.back")} onClick={onBack}><Icon name="arrowLeft" /><span className="web-label-long">{returnTitle ? t("web.backTo", { title: returnTitle }) : t("web.back")}</span><span className="web-label-short">{t("web.back")}</span></button>
    <span className="reader-head-divider" />
    <span className="web-host" data-tone={tone ?? (insecure ? "warn" : undefined)} data-live={live || undefined} title={t("web.actualAddress")}><Icon name={insecure || tone ? "warning" : "lock"} /><span>{webHost(page)}</span></span>
  </>;
}

export function WebReadingActions({ page, onCollect, collecting = false, view = "reader", onRead }: Pick<WebBarProps, "page" | "onCollect" | "collecting" | "view" | "onRead">) {
  const { t } = useTranslation("reader");
  const url = page.info?.actual_url ?? page.url;
  const actions = useContext(WebLinksContext);
  const saved = page.info && !page.info.temporary;
  return <>
    <div className="web-mode" role="group" aria-label={t("web.viewLabel")}><button type="button" aria-pressed={view === "reader"} onClick={onRead}>{t("web.readerView")}</button><button type="button" aria-pressed={view === "web"} onClick={() => { if (view !== "web") actions?.open(url, "", "web"); }}>{t("web.originalPage")}</button></div>
    <button type="button" className="web-tool" aria-label={t("web.openInBrowser")} onClick={() => actions ? actions.open(url, "", "browser") : void openExternal(url)}><Icon name="external" /><span className="web-label-long">{t("web.systemBrowser")}</span></button>
    {page.info ? <button type="button" className="web-tool" data-saved={saved || undefined} aria-label={saved ? t("web.saved") : t("web.save")} disabled={saved || collecting} aria-busy={collecting} onClick={onCollect}><Icon name="bookmark" /><span className="web-label-long">{saved ? t("web.saved") : t("web.save")}</span></button> : null}
  </>;
}

export function WebReadingBar({ page, returnTitle, onBack, onCollect, collecting = false, tone, view = "reader", onRead }: WebBarProps) {
  return <div className="web-bar" data-tauri-drag-region="deep">
    <WebReadingNav page={page} returnTitle={returnTitle} onBack={onBack} tone={tone} />
    <span className="web-grow" />
    <WebReadingActions page={page} onCollect={onCollect} collecting={collecting} view={view} onRead={onRead} />
  </div>;
}

export function WebOrigin({ info, extractedAt, url, onExtract, extracting = false }: {
  info?: OpenWebLinkView; extractedAt?: number | null; url: string; onExtract: () => void; extracting?: boolean;
}) {
  const { t } = useTranslation("reader");
  const actions = useContext(WebLinksContext);
  const extracted = extractedAt === undefined ? info?.web_extracted_at : extractedAt;
  const hasWindow = actions?.hasWindow?.(url);
  return <div className="web-origin">
    {info ? <span className="web-badge">{info.temporary ? t("web.origin.temporary") : t("web.origin.inLibrary")}</span> : null}
    {extracted != null ? <>
      <span><Trans t={t} i18nKey="web.origin.extractedAt" components={{ rel: <RelativeTime ms={extracted} /> }} /></span>
      {hasWindow ? <button type="button" className="link" disabled={extracting} aria-busy={extracting} onClick={onExtract}>{t("web.origin.reextract")}</button> : <><span>{t("web.origin.openFirst")}</span><button type="button" className="link" onClick={() => actions?.open(url, "", "web")}>{t("web.openOriginal")}</button></>}
    </> : info ? <span>{info.layer === "UserRule"
      ? t("web.origin.statsHost", { layer: t(`web.layer.${info.layer}`), host: info.actual_host, chars: info.text_chars.toLocaleString(locale()), images: info.image_count })
      : t("web.origin.stats", { layer: t(`web.layer.${info.layer}`), chars: info.text_chars.toLocaleString(locale()), images: info.image_count })}</span> : null}
    {info?.temporary ? <span>{t("web.origin.autoSave")}</span> : null}
  </div>;
}

export function WebFailure({ page, onRetry }: { page: WebPage & { error: FulltextError }; onRetry: () => void }) {
  const { t } = useTranslation("reader");
  const waf = page.error.kind === "Waf";
  const actions = useContext(WebLinksContext);
  return <div className="web-failure" role="status">
    <h2>{t("web.failure.title")}</h2>
    <p>{t("web.failure.notAutoLoaded", { detail: webFailureReasons[page.error.kind].detail })}</p>
    <div className="web-failure-actions">
      <button type="button" className="btn primary" onClick={() => actions?.open(page.url, "", "web")}>{waf ? t("web.failure.openToVerify") : t("web.openOriginal")}</button>
      <button type="button" className="btn" onClick={() => actions ? actions.open(page.url, "", "browser") : void openExternal(page.url)}>{t("web.openInBrowser")}</button>
      <button type="button" className="btn ghost" onClick={onRetry}>{t("web.failure.retry")}</button>
    </div>
    {waf ? <small>{t("web.failure.verifyNote")}</small> : null}
    <small><Trans t={t} i18nKey="web.failure.causes" values={{ host: webHost(page) }}
      components={{ rule: <button type="button" className="link" onClick={() => actions?.editRule?.(webHost(page), page.info?.actual_url ?? page.url)} /> }} /></small>
  </div>;
}

export function WebFirstTip({ page }: { page: WebPage }) {
  const { t } = useTranslation("reader");
  const query = useQuery(settingsSchemaOptions());
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(settingsMutationOptions(client));
  const [closed, setClosed] = useState(false);
  const [never, setNever] = useState(false);
  const [failed, setFailed] = useState(false);
  if (closed || query.data?.values.find(v => v.id === "web_first_tip")?.value !== "1") return null;
  const dismiss = async () => {
    if (never) {
      const result = await run([{ id: "web_first_tip", value: "0" }]);
      if (result.status !== "done" || result.data.failure) { setFailed(true); return; }
    }
    setClosed(true);
  };
  return <div className="web-tip web-first-tip" role="note">
    <div className="web-tip-text"><Trans t={t} i18nKey="web.firstTip.text" values={{ host: webHost(page) }} components={{ strong: <strong /> }} /></div>
    <div className="web-tip-actions"><label><input type="checkbox" checked={never} disabled={writing} onChange={event => setNever(event.target.checked)} />{t("web.firstTip.never")}</label><button type="button" className="btn sm" disabled={writing} aria-busy={writing} onClick={() => void dismiss()}>{t("web.firstTip.ok")}</button></div>
    {failed ? <small role="alert">{t("web.firstTip.failed")}</small> : null}
  </div>;
}
