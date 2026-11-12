import { useContext, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { settingsMutationOptions, settingsSchemaOptions } from "../../settings/settingsQuery";
import { useSettingsWrite } from "../../settings/editState";
import { WebLinksContext, webFailureReasons } from "../../lib/webLinks";
import type { FulltextError, OpenWebLinkView, WebLinkLayer } from "../../bindings";
import { openExternal } from "../../lib/api";
import { RelativeTime } from "../../components/RelativeTime";
import { Icon } from "../../components/Icon";

export type WebPage = { url: string; info?: OpenWebLinkView; error?: FulltextError };
const layers: Record<WebLinkLayer, string> = {
  UserRule: "按你的站点规则抽取", BuiltinRule: "按内置站点规则抽取", General: "通用识别",
  Fallback: "通用识别（回退）", Container: "页面里的 article / main",
};
function webHost(page: WebPage) {
  return new URL(page.info?.actual_url ?? page.url).hostname;
}

type WebBarProps = {
  page: WebPage; returnTitle?: string; onBack: () => void; onCollect?: () => void; collecting?: boolean;
  tone?: "warn" | "danger"; view?: "reader" | "web"; onRead?: () => void;
};

export function WebReadingNav({ page, returnTitle, onBack, tone, live = false }: Pick<WebBarProps, "page" | "returnTitle" | "onBack" | "tone"> & { live?: boolean }) {
  const url = page.info?.actual_url ?? page.url;
  const insecure = new URL(url).protocol !== "https:";
  return <>
    <button type="button" className="web-tool web-back" aria-label="返回" onClick={onBack}><Icon name="arrowLeft" /><span className="web-label-long">{returnTitle ? `回到「${returnTitle}」` : "返回"}</span><span className="web-label-short">返回</span></button>
    <span className="reader-head-divider" />
    <span className="web-host" data-tone={tone ?? (insecure ? "warn" : undefined)} data-live={live || undefined} title="实际地址"><Icon name={insecure || tone ? "warning" : "lock"} /><span>{webHost(page)}</span></span>
  </>;
}

export function WebReadingActions({ page, onCollect, collecting = false, view = "reader", onRead }: Pick<WebBarProps, "page" | "onCollect" | "collecting" | "view" | "onRead">) {
  const url = page.info?.actual_url ?? page.url;
  const actions = useContext(WebLinksContext);
  const saved = page.info && !page.info.temporary;
  return <>
    <div className="web-mode" role="group" aria-label="看法"><button type="button" aria-pressed={view === "reader"} onClick={onRead}>阅读视图</button><button type="button" aria-pressed={view === "web"} onClick={() => { if (view !== "web") actions?.open(url, "", "web"); }}>原网页</button></div>
    <button type="button" className="web-tool" aria-label="在系统浏览器打开" onClick={() => actions ? actions.open(url, "", "browser") : void openExternal(url)}><Icon name="external" /><span className="web-label-long">系统浏览器</span></button>
    {page.info ? <button type="button" className="web-tool" data-saved={saved || undefined} aria-label={saved ? "已收录" : "收录"} disabled={saved || collecting} aria-busy={collecting} onClick={onCollect}><Icon name="bookmark" /><span className="web-label-long">{saved ? "已收录" : "收录"}</span></button> : null}
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
  const actions = useContext(WebLinksContext);
  const extracted = extractedAt === undefined ? info?.web_extracted_at : extractedAt;
  const hasWindow = actions?.hasWindow?.(url);
  return <div className="web-origin">
    {info ? <span className="web-badge">{info.temporary ? "临时打开 · 不在库里" : "已收录到库"}</span> : null}
    {extracted != null ? <>
      <span>从原网页抽取 · <RelativeTime ms={extracted} /> · 页面后来又加载了内容时可以</span>
      {hasWindow ? <button type="button" className="link" disabled={extracting} aria-busy={extracting} onClick={onExtract}>重新抽取</button> : <><span>先打开原网页</span><button type="button" className="link" onClick={() => actions?.open(url, "", "web")}>打开原网页</button></>}
    </> : info ? <span>{layers[info.layer]}{info.layer === "UserRule" ? `（${info.actual_host}）` : ""} · {info.text_chars.toLocaleString("en-US")} 字 · {info.image_count} 张图</span> : null}
    {info?.temporary ? <span>引用或发到对话时会自动收录</span> : null}
  </div>;
}

export function WebFailure({ page, onRetry }: { page: WebPage & { error: FulltextError }; onRetry: () => void }) {
  const waf = page.error.kind === "Waf";
  const actions = useContext(WebLinksContext);
  return <div className="web-failure" role="status">
    <h2>没能提取这个网页的正文</h2>
    <p>{webFailureReasons[page.error.kind].detail}不会自动加载原网页。</p>
    <div className="web-failure-actions">
      <button type="button" className="btn primary" onClick={() => actions?.open(page.url, "", "web")}>{waf ? "打开原网页通过验证" : "打开原网页"}</button>
      <button type="button" className="btn" onClick={() => actions ? actions.open(page.url, "", "browser") : void openExternal(page.url)}>在系统浏览器打开</button>
      <button type="button" className="btn ghost" onClick={onRetry}>重试</button>
    </div>
    {waf ? <small>在原网页里通过人机验证后，RRSS 会自己试着抽取。验证由你本人完成，RRSS 不做自动化。</small> : null}
    <small>常见原因还有：页面靠脚本生成正文、需要登录、地址指向首页而不是文章。也可以 <button type="button" className="link" onClick={() => actions?.editRule?.(webHost(page), page.info?.actual_url ?? page.url)}>为 {webHost(page)} 写一条规则</button>。</small>
  </div>;
}

export function WebFirstTip({ page }: { page: WebPage }) {
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
    <div className="web-tip-text"><strong>这是外部网页，内容来自 {webHost(page)}。</strong>RRSS 只做了抽取和排版，没有核实内容。不要轻信页面里的链接，也不要在网页里输入密码、验证码或支付信息。</div>
    <div className="web-tip-actions"><label><input type="checkbox" checked={never} disabled={writing} onChange={event => setNever(event.target.checked)} />不再显示</label><button type="button" className="btn sm" disabled={writing} aria-busy={writing} onClick={() => void dismiss()}>知道了</button></div>
    {failed ? <small role="alert">没能保存提示设置，请重试。</small> : null}
  </div>;
}
