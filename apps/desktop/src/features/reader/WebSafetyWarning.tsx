import { WebReadingBar } from "./WebReading";
import type { WebWarning } from "./useWebLinks";

export function WebSafetyWarning({ warning, onCancel, onContinue }: {
  warning: WebWarning; onCancel: () => void; onContinue: () => void;
}) {
  const danger = warning.safety.warnings.some(w => w.kind !== "NonHttps");
  return <>
      <WebReadingBar tone={danger ? "danger" : "warn"} page={{ url: warning.href }} onBack={onCancel} />
      <div className="drawer-body"><div className="reader-column web-column">
        <div className="web-tip web-warning" data-tone={danger ? "danger" : "warn"} role="alert">
          <div className="web-tip-text">{warning.safety.warnings.map((item, index) => <div key={index}>
            {item.kind === "MixedScript" ? <><strong>这个地址的域名混用了不同文字，可能是仿冒网站。</strong><p>看起来是 <code>{item.unicode}</code>，实际域名是 <code>{item.punycode}</code>。</p></>
              : item.kind === "HostMismatch" ? <><strong>链接文字和实际地址不一致。</strong><p>链接里写的是 <code>{item.text_host}</code>，实际会去 <code>{warning.safety.actual_host}</code>。</p></>
                : <><strong>这个网页没有使用 HTTPS。</strong><p>内容在传输中可能被改写，阅读视图里看到的未必是站点原样。不要在原网页里提交任何信息。</p></>}
          </div>)}</div>
          <div className="web-tip-actions"><button type="button" className="btn primary" onClick={onCancel}>不打开</button><button type="button" className="btn ghost" onClick={onContinue}>{danger ? "仍然打开" : "继续打开"}</button></div>
          <small>这类提示不能关闭，和「首次提示」无关。</small>
        </div>
      </div></div>
  </>;
}
