import type { WebVerifyStateEvent } from "../../bindings";

/** 过盾进度：画在原网页顶栏下方、网页区域之外。不自动切换视图，也不关掉原网页。 */
export function WebVerifyStrip({ event, busy, onAbandon, onExtract, onRead }: {
  event: WebVerifyStateEvent; busy: boolean; onAbandon: () => void; onExtract: () => void; onRead: () => void;
}) {
  const ready = event.state === "Extracted", miss = event.state === "NotExtracted", wait = event.state === "WaitingVerification";
  const done = ready ? 3 : wait ? 1 : 2;
  return <div className="web-verify-strip" data-tone={ready ? "accent" : miss || wait ? "warn" : undefined} aria-label="原网页抽取进度">
    <span className="web-verify-steps" aria-hidden="true">{[0, 1, 2].map(i => <i key={i} data-done={i < done || undefined} />)}</span>
    <span className="web-verify-text" role="status">{ready
      ? <><b>已从原网页抽到正文</b> · 《{event.title}》· {event.text_chars?.toLocaleString("en-US")} 字 · {event.image_count} 张图。原网页还留着，切回去不会重新加载。</>
      : miss ? <><b>没抽到像文章的正文</b>{event.reason ? ` · ${event.reason}` : ""}</>
      : wait ? <><b>等待验证</b> · 在下面的网页里完成验证后，RRSS 会自己抽取当前页。</>
      : <><b>正在预抽取</b> · 页面已加载，RRSS 正在试着抽取。</>}</span>
    <span className="web-verify-actions">
      {ready ? <>
        <button type="button" className="btn sm primary" disabled={busy || event.article_id == null} onClick={onRead}>切到阅读视图</button>
        <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={onExtract}>页面还在加载，稍后重抽</button>
      </> : miss ? <button type="button" className="btn sm primary" disabled={busy} aria-busy={busy} onClick={onExtract}>现在再抽一次</button> : null}
      {!ready ? <button type="button" className="btn sm ghost" onClick={onAbandon}>放弃</button> : null}
    </span>
  </div>;
}
