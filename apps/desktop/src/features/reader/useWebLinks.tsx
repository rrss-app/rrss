import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useWebWindows } from "./useWebWindows";
import type { CitationView, LinkSafety, WebVerifyStateEvent } from "../../bindings";
import { api, copyText, isSafeHttpUrl, openExternal } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { webFailureReasons, type WebLinkNotice, type WebLinkTarget, type WebOpenMode } from "../../lib/webLinks";
import { FloatingMenu, type MenuAnchor } from "../../components/FloatingMenu";
import { settingsSchemaOptions } from "../../settings/settingsQuery";

type Action = WebLinkTarget & { mode: WebOpenMode };
export type WebWarning = Action & { safety: LinkSafety };
export function useWebLinks({ openReader, openExtracted, prepareCitation, notify, showWarning, closeWarning }: {
  openReader: (url: string) => void;
  openExtracted: (event: WebVerifyStateEvent) => void;
  prepareCitation: () => (chip: CitationView) => void;
  notify: (notice: WebLinkNotice) => void;
  showWarning: () => void; closeWarning: () => void;
}) {
  const client = useQueryClient();
  const callbacks = useRef({ openReader, prepareCitation, notify, showWarning });
  callbacks.current = { openReader, prepareCitation, notify, showWarning };
  const generation = useRef(0);
  const [warning, setWarning] = useState<WebWarning | null>(null);
  const [menu, setMenu] = useState<(WebLinkTarget & { anchor: MenuAnchor; opener: HTMLAnchorElement }) | null>(null);
  useEffect(() => () => { ++generation.current; }, []);
  const cancel = useCallback(() => { ++generation.current; setWarning(null); }, []);
  const windows = useWebWindows({ show: showWarning, close: closeWarning, notify, openExtracted, openLink: (url, text, mode) => { void openContentLink(url, text, mode); } });
  const windowBegin = useRef(windows.begin); windowBegin.current = windows.begin;
  const perform = useCallback(async (action: Action) => {
    const { openReader, prepareCitation, notify } = callbacks.current;
    const ticket = ++generation.current;
    try {
      if (action.mode === "web") { await windowBegin.current(action.href); return; }
      if (action.mode === "reader") { openReader(action.href); return; }
      if (action.mode === "browser") { await openExternal(action.href); return; }
      const add = prepareCitation();
      notify({ message: "正在抓取网页并添加文章引用…", holdMs: 60_000 });
      const info = await api.openWebLink(action.href);
      if (ticket !== generation.current) return;
      const chip = await api.citeArticle(info.article_id);
      if (ticket !== generation.current) return;
      add(chip);
      notify({ message: "已添加文章引用" });
    } catch (error) {
      if (ticket !== generation.current) return;
      const kind = error && typeof error === "object" && "kind" in error ? error.kind : null;
      notify({ message: typeof kind === "string" && Object.prototype.hasOwnProperty.call(webFailureReasons, kind) ? `引用链接失败：${webFailureReasons[kind as keyof typeof webFailureReasons].summary}` : formatNotice("链接操作失败：", error) });
    }
  }, []);
  // 默认打开方式集中在这里；显式菜单动作覆盖默认值，三种动作共用安全判定。
  const openContentLink = useCallback(async (href: string, text = "", mode?: WebOpenMode) => {
    if (!isSafeHttpUrl(href)) return;
    const ticket = ++generation.current;
    try {
      const schema = await client.ensureQueryData(settingsSchemaOptions());
      const action: Action = { href, text, mode: mode ?? ((schema.values.find(v => v.id === "web_open_mode")?.value as "reader" | "web" | "browser") ?? "reader") };
      const safety = await api.linkSafety(href, text);
      if (ticket !== generation.current) return;
      if (safety.warnings.length) { setWarning({ ...action, safety }); callbacks.current.showWarning(); }
      else void perform(action);
    } catch (error) {
      if (ticket === generation.current) callbacks.current.notify({ message: formatNotice("无法检查链接：", error) });
    }
  }, [client, perform]);
  const context = useMemo(() => ({ open: openContentLink, menu: (target: WebLinkTarget, anchor: MenuAnchor, opener: HTMLAnchorElement) => setMenu({ ...target, anchor, opener }) }), [openContentLink]);
  const run = (mode: WebOpenMode) => { if (menu) void openContentLink(menu.href, menu.text, mode); setMenu(null); };
  return {
    warning, windows, cancel: () => { cancel(); windows.cancel(); }, openContentLink,
    proceed: () => { if (!warning) return; const action = warning; if (action.mode === "web") setWarning(null); else closeWarning(); void perform(action); },
    context: { ...context, hasWindow: windows.hasWindow, extract: windows.extractUrl },
    menu: menu ? <FloatingMenu anchor={menu.anchor} restoreFocusTo={menu.opener} boundary={menu.opener.closest<HTMLElement>(".drawer-body, .scroll")} label="链接操作" className="reader-context-menu web-link-menu" onClose={() => setMenu(null)}>
      <div className="web-link-address">{menu.href}</div>
      <button type="button" role="menuitem" onClick={() => run("reader")}>在 RRSS 里打开<small>点击</small></button>
      <button type="button" role="menuitem" onClick={() => run("web")}>打开原网页</button>
      <button type="button" role="menuitem" onClick={() => run("browser")}>在系统浏览器打开</button>
      <hr role="separator" />
      <button type="button" role="menuitem" onClick={() => { void copyText(menu.href).catch(error => notify({ message: formatNotice("复制失败：", error) })); setMenu(null); }}>复制链接</button>
      <button type="button" role="menuitem" onClick={() => run("cite")}>引用链接</button>
    </FloatingMenu> : null,
  };
}
