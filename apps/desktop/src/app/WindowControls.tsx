import { useEffect, useState } from "react";
import type { WindowControlsStyle } from "../bindings";
import { api, windowControls } from "../lib/api";
import { useTranslation } from "../i18n";

/** 10px 细线图标：Windows 用 1px 直角线，GNOME 用 1.6px 圆头线（样式里区分）。 */
const GLYPHS: Record<WindowControlsStyle, { minimize: string; maximize: string; restore: string; close: string }> = {
  windows: {
    minimize: "M0 5.5h10",
    maximize: "M.5 .5h9v9h-9z",
    restore: "M2.5 2.5V.5h7v7h-2M.5 2.5h7v7h-7z",
    close: "M0 0l10 10M10 0 0 10",
  },
  gnome: {
    minimize: "M2 7.5h6",
    maximize: "M2 2h6v6H2z",
    restore: "M1.5 3.5h5v5h-5zM3.5 1.5h5v5",
    close: "M2 2l6 6M8 2 2 8",
  },
};

const run = (action: () => Promise<unknown>) => { action().catch((error) => console.error("window control failed", error)); };

/**
 * 自绘标题栏的窗口三键。主窗口没有系统标题栏，三键固定在右上角、所有叠层之上；
 * 宿主告诉前端按哪个平台的样子画，根元素记 `data-chrome`，右上角有内容的顶栏据此用 `--window-controls-w` 让位。
 * 没有宿主（浏览器预览）时什么也不画。
 */
export function WindowControls() {
  const { t } = useTranslation("shell");
  const [style, setStyle] = useState<WindowControlsStyle | null>(null);
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const sync = () => {
      windowControls.isMaximized().then((value) => { if (!disposed) setMaximized(value); }).catch(() => {});
    };
    api.windowChrome().then((chrome) => {
      if (disposed || !chrome) return;
      document.documentElement.dataset.chrome = chrome.controls;
      setStyle(chrome.controls);
      sync();
      return windowControls.onResized(sync).then((stop) => { if (disposed) stop(); else unlisten = stop; });
    }).catch((error) => console.error("window chrome unavailable", error));
    return () => {
      disposed = true;
      unlisten?.();
      delete document.documentElement.dataset.chrome;
    };
  }, []);
  if (!style) return null;
  const glyph = GLYPHS[style];
  const sizeLabel = maximized ? t("window.restore") : t("window.maximize");
  return <div className="window-controls" data-style={style}>
    <button type="button" tabIndex={-1} aria-label={t("window.minimize")} title={t("window.minimize")} onClick={() => run(windowControls.minimize)}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d={glyph.minimize} /></svg>
    </button>
    <button type="button" tabIndex={-1} aria-label={sizeLabel} title={sizeLabel} onClick={() => run(windowControls.toggleMaximize)}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d={maximized ? glyph.restore : glyph.maximize} /></svg>
    </button>
    <button type="button" tabIndex={-1} className="window-close" aria-label={t("window.closeWindow")} title={t("window.close")} onClick={() => run(windowControls.close)}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d={glyph.close} /></svg>
    </button>
  </div>;
}
