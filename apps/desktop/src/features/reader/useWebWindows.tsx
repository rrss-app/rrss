import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, onWebClosed, onWebDownloadBlocked, onWebEscape, onWebNavigated, onWebVerifyState } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { useSettingsWrite } from "../../settings/editState";
import { settingsMutationOptions, settingsSchemaOptions } from "../../settings/settingsQuery";
import { networkProxyQueryOptions, routeSummary } from "../../settings/networkProxyQuery";
import { FloatingMenu, type MenuAnchor } from "../../components/FloatingMenu";
import { Icon } from "../../components/Icon";
import { WebReadingBar, WebReadingNav } from "./WebReading";
import { WebVerifyStrip } from "./WebVerifyStrip";
import type { WebVerifyStateEvent, WebViewBounds } from "../../bindings";
import type { WebLinkNotice, WebOpenMode } from "../../lib/webLinks";

type Panel = { url: string; phase: "tip" | "opening" | "opened" | "error"; error?: string };
type Live = { label: string; openedUrl: string; url: string };
/** 宿主 `web_view_action` 的白名单动作。 */
export type WebViewAction = "back" | "forward" | "reload" | "copy" | "open" | "disable-js" | "clear" | "close";
const ACTION_DONE: Partial<Record<WebViewAction, string>> = {
  copy: "已复制当前地址", "disable-js": "已禁用本站 JavaScript，重新打开原网页后生效。", clear: "已清空原网页数据",
};
const sameBounds = (a: WebViewBounds, b: WebViewBounds) => a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

export function useWebWindows({ show, close, notify, openLink, openExtracted }: {
  show: () => void; close: () => void; notify: (notice: WebLinkNotice) => void;
  openExtracted: (event: WebVerifyStateEvent) => void;
  openLink: (url: string, text?: string, mode?: WebOpenMode) => void;
}) {
  const callbacks = useRef({ show, close, notify, openLink, openExtracted });
  callbacks.current = { show, close, notify, openLink, openExtracted };
  const client = useQueryClient();
  const [panel, setPanel] = useState<Panel | null>(null);
  const [current, setCurrent] = useState<Live | null>(null);
  const [embedded, setEmbedded] = useState(false);
  const [verification, setVerification] = useState<Record<string, WebVerifyStateEvent>>({});
  const [extracting, setExtracting] = useState<string | null>(null);
  const panelRef = useRef<Panel | null>(null);
  const liveRef = useRef<Live | null>(null);
  const known = useRef(new Map<string, { url: string }>());
  const closed = useRef(new Set<string>());
  const abandoned = useRef(new Map<string, string>());
  const generation = useRef(0);
  const epoch = useRef(0);
  const chain = useRef(Promise.resolve());
  const opening = useRef(false);
  const replacing = useRef<string | null>(null);
  const sceneOn = useRef(false);
  const coveredRef = useRef(false);
  const slotReady = useRef(false);
  const latestBounds = useRef<WebViewBounds | null>(null);
  const placedBounds = useRef<WebViewBounds | null>(null);
  const boundsWait = useRef<((bounds: WebViewBounds | null) => void) | null>(null);
  const visibleWanted = useRef<boolean | null>(null);
  const embeddedRef = useRef(false);
  const extractingRef = useRef<string | null>(null);
  const showPanel = (next: Panel | null) => { panelRef.current = next; setPanel(next); };
  const clearVerification = useCallback((label: string) => {
    setVerification(old => { if (!(label in old)) return old; const next = { ...old }; delete next[label]; return next; });
  }, []);
  const syncVisible = useCallback(() => {
    const live = liveRef.current;
    const want = sceneOn.current && panelRef.current?.phase === "opened" && !coveredRef.current && !!live && !closed.current.has(live.label);
    if (embeddedRef.current !== want) { embeddedRef.current = want; setEmbedded(want); }
    if (!live || closed.current.has(live.label) || visibleWanted.current === want) return;
    visibleWanted.current = want;
    void api.setWebViewVisible(live.label, want).catch(error => callbacks.current.notify({ message: formatNotice("原网页显示失败：", error) }));
  }, []);
  const receiveVerification = useCallback((event: WebVerifyStateEvent) => {
    const live = liveRef.current;
    if (!live || closed.current.has(event.label) || live.label !== event.label || live.url !== event.url || abandoned.current.get(event.label) === event.url) return;
    setVerification(old => ({ ...old, [event.label]: event }));
  }, []);
  const dropWait = useCallback(() => {
    const wait = boundsWait.current;
    boundsWait.current = null;
    wait?.(null);
  }, []);
  useEffect(() => {
    let disposed = false;
    const requestGeneration = generation;
    const unlisten: (() => void)[] = [];
    const subscriptions = [
      onWebVerifyState(receiveVerification),
      onWebNavigated(event => {
        if (closed.current.has(event.label)) return;
        known.current.set(event.label, event);
        const live = liveRef.current;
        if (!live || live.label !== event.label || live.url === event.url) return;
        ++epoch.current;
        abandoned.current.delete(event.label);
        clearVerification(event.label);
        const next = { ...live, url: event.url };
        liveRef.current = next;
        setCurrent(next);
      }),
      onWebClosed(({ label }) => {
        closed.current.add(label);
        known.current.delete(label);
        abandoned.current.delete(label);
        ++epoch.current;
        clearVerification(label);
        const currentLabel = liveRef.current?.label === label;
        const replacingThis = replacing.current === label;
        if (currentLabel) {
          liveRef.current = null;
          setCurrent(null);
          visibleWanted.current = null;
          if (embeddedRef.current) { embeddedRef.current = false; setEmbedded(false); }
        }
        // 换地址时宿主先关掉旧的；这次关闭不是用户离开，场景继续等新的原网页。
        if (!currentLabel || replacingThis) return;
        if (panelRef.current?.phase === "opening" || panelRef.current?.phase === "opened") {
          panelRef.current = null;
          setPanel(null);
          callbacks.current.close();
        }
      }),
      onWebEscape(({ label }) => {
        const live = liveRef.current;
        if (!live || live.label !== label || closed.current.has(label)) return;
        callbacks.current.close();
        callbacks.current.openLink(live.url, "", "reader");
      }),
      onWebDownloadBlocked(({ url }) => callbacks.current.notify({ message: "已拦截下载，可交给系统浏览器下载", actionLabel: "系统浏览器", onAction: () => callbacks.current.openLink(url, "", "browser") })),
    ];
    for (const subscription of subscriptions) void subscription.then(off => { if (disposed) off(); else unlisten.push(off); }).catch(error => { if (!disposed) callbacks.current.notify({ message: formatNotice("无法接收原网页事件：", error) }); });
    return () => { disposed = true; ++requestGeneration.current; dropWait(); unlisten.forEach(off => off()); };
  }, [clearVerification, dropWait, receiveVerification]);
  const pushBounds = useCallback((bounds: WebViewBounds) => {
    const live = liveRef.current;
    if (!live || panelRef.current?.phase !== "opened" || closed.current.has(live.label)) return;
    if (placedBounds.current && sameBounds(placedBounds.current, bounds)) return;
    placedBounds.current = bounds;
    void api.setWebViewBounds(live.label, bounds).catch(error => callbacks.current.notify({ message: formatNotice("原网页位置更新失败：", error) }));
  }, []);
  const reportBounds = useCallback((bounds: WebViewBounds) => {
    slotReady.current = true;
    const wait = boundsWait.current;
    const changed = !latestBounds.current || !sameBounds(latestBounds.current, bounds);
    latestBounds.current = bounds;
    if (wait) { boundsWait.current = null; wait(bounds); return; }
    if (changed) pushBounds(bounds);
  }, [pushBounds]);
  const clearSlotBounds = useCallback(() => {
    slotReady.current = false;
    latestBounds.current = null;
    dropWait();
  }, [dropWait]);
  const takeBounds = (ticket: number) => {
    if (ticket !== generation.current) return Promise.resolve(null);
    if (slotReady.current && latestBounds.current) return Promise.resolve(latestBounds.current);
    return new Promise<WebViewBounds | null>(resolve => { boundsWait.current = resolve; });
  };
  const reusable = (url: string) => {
    const live = liveRef.current;
    return !!live && !closed.current.has(live.label) && (live.openedUrl === url || live.url === url);
  };
  const runLaunch = async (url: string, ticket: number) => {
    if (ticket !== generation.current) return;
    opening.current = true;
    try {
      if (reusable(url)) {
        showPanel({ url: liveRef.current!.url, phase: "opened" });
        callbacks.current.show();
        syncVisible();
        if (latestBounds.current) pushBounds(latestBounds.current);
        return;
      }
      showPanel({ url, phase: "opening" });
      callbacks.current.show();
      const bounds = await takeBounds(ticket);
      if (!bounds || ticket !== generation.current || !sceneOn.current || panelRef.current?.url !== url) return;
      replacing.current = liveRef.current?.label ?? null;
      let label: string;
      try { label = await api.openWebView(url, bounds); }
      catch (error) {
        replacing.current = null;
        if (ticket === generation.current) showPanel({ url, phase: "error", error: formatNotice("", error) });
        return;
      }
      const stale = ticket !== generation.current || !sceneOn.current || panelRef.current?.url !== url;
      if (stale) {
        replacing.current = null;
        await api.closeWebView(label).catch(() => {});
        return;
      }
      const navigated = known.current.get(label)?.url ?? url;
      const live = { label, openedUrl: url, url: navigated };
      liveRef.current = live;
      setCurrent(live);
      placedBounds.current = bounds;
      replacing.current = null;
      visibleWanted.current = null;
      showPanel({ url: navigated, phase: "opened" });
      syncVisible();
      if (latestBounds.current && !sameBounds(latestBounds.current, bounds)) pushBounds(latestBounds.current);
    } finally { opening.current = false; }
  };
  const launch = (url: string) => {
    const ticket = ++generation.current;
    const run = chain.current.then(() => runLaunch(url, ticket));
    chain.current = run.then(() => undefined, () => undefined);
    return run;
  };
  const begin = async (url: string) => {
    const ticket = ++generation.current;
    showPanel({ url, phase: "opening" });
    callbacks.current.show();
    try {
      const schema = await client.ensureQueryData(settingsSchemaOptions());
      if (ticket !== generation.current) return;
      if (schema.values.find(v => v.id === "web_original_tip")?.value === "1") showPanel({ url, phase: "tip" });
      else await launch(url);
    } catch (error) {
      if (ticket === generation.current) showPanel({ url, phase: "error", error: formatNotice("", error) });
    }
  };
  const extract = async () => {
    const live = liveRef.current;
    if (!live || extractingRef.current || closed.current.has(live.label)) return null;
    const ticket = epoch.current;
    const url = live.url;
    extractingRef.current = live.label;
    setExtracting(live.label);
    try {
      const result = await api.extractWebView(live.label);
      if (ticket !== epoch.current || closed.current.has(live.label) || liveRef.current?.label !== live.label || liveRef.current.url !== url || result.label !== live.label || result.url !== url) return null;
      receiveVerification(result);
      return result;
    } catch (error) {
      if (ticket === epoch.current && liveRef.current?.url === url) callbacks.current.notify({ message: formatNotice("抽取失败：", error) });
      return null;
    } finally { extractingRef.current = null; setExtracting(null); }
  };
  const matches = (url: string) => reusable(url);
  const release = useCallback(() => {
    const live = liveRef.current;
    if (!live && !boundsWait.current && !opening.current) return;
    ++generation.current;
    dropWait();
    opening.current = false;
    if (!live) return;
    const label = live.label;
    liveRef.current = null;
    setCurrent(null);
    closed.current.add(label);
    visibleWanted.current = null;
    if (embeddedRef.current) { embeddedRef.current = false; setEmbedded(false); }
    clearVerification(label);
    panelRef.current = null;
    setPanel(null);
    void api.closeWebView(label).catch(error => callbacks.current.notify({ message: formatNotice("原网页关闭失败：", error) }));
  }, [clearVerification, dropWait]);
  return {
    verification, extracting, embedded, panel, current, extract,
    hasWindow: matches,
    extractUrl: async (url: string) => matches(url) ? extract() : null,
    abandon: () => {
      const live = liveRef.current;
      if (!live) return;
      ++epoch.current;
      abandoned.current.set(live.label, live.url);
      clearVerification(live.label);
    },
    readExtracted: (event: WebVerifyStateEvent) => { callbacks.current.close(); callbacks.current.openExtracted(event); },
    begin, launch, release,
    act: async (action: WebViewAction) => {
      const live = liveRef.current;
      if (!live || closed.current.has(live.label)) return;
      try {
        await api.webViewAction(live.label, action);
        const done = ACTION_DONE[action];
        if (done) callbacks.current.notify({ message: done });
      } catch (error) { callbacks.current.notify({ message: formatNotice("原网页操作失败：", error) }); }
    },
    backToReader: (url: string) => { callbacks.current.close(); callbacks.current.openLink(url, "", "reader"); },
    cancel: () => { ++generation.current; dropWait(); showPanel(null); syncVisible(); },
    sceneMounted: (mounted: boolean) => {
      sceneOn.current = mounted;
      if (!mounted) { slotReady.current = false; latestBounds.current = null; dropWait(); }
      syncVisible();
    },
    setCovered: (covered: boolean) => { if (coveredRef.current === covered) return; coveredRef.current = covered; syncVisible(); },
    reportBounds, clearSlotBounds,
  };
}

const PAUSED = "菜单、对话框、toast 打开时，原生网页会挡住它们，所以先隐藏；关闭菜单后立刻恢复，不重新加载。";
const MENU: [WebViewAction | "extract", string, Parameters<typeof Icon>[0]["name"]][] = [
  ["copy", "复制当前地址", "copy"], ["open", "在系统浏览器打开", "external"], ["extract", "从当前页抽取正文", "fileText"],
  ["disable-js", "禁用本站 JavaScript", "stop"], ["clear", "清空原网页数据", "trash"], ["close", "关闭原网页", "close"],
];

export function WebWindowScene({ controller, covered, onBack, onOpenLink, onProxySettings }: {
  controller: ReturnType<typeof useWebWindows>; covered: boolean; onBack: () => void;
  onOpenLink: (url: string, text?: string, mode?: WebOpenMode) => void; onProxySettings: () => void;
}) {
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(settingsMutationOptions(client));
  const network = useQuery(networkProxyQueryOptions());
  const slotRef = useRef<HTMLDivElement>(null);
  const menuButton = useRef<HTMLButtonElement>(null);
  const controllerRef = useRef(controller);
  controllerRef.current = controller;
  const [never, setNever] = useState(false);
  const [error, setError] = useState("");
  const [menu, setMenu] = useState<MenuAnchor | null>(null);
  const { panel, current } = controller;
  const watching = panel?.phase === "opening" || panel?.phase === "opened";
  const hidden = covered || !!menu;
  useLayoutEffect(() => {
    controllerRef.current.sceneMounted(true);
    return () => controllerRef.current.sceneMounted(false);
  }, []);
  // 「⋯」菜单也是盖在原生网页上的一层：打开期间先收起网页。
  useLayoutEffect(() => { controllerRef.current.setCovered(hidden); }, [hidden]);
  useLayoutEffect(() => {
    const node = slotRef.current;
    if (!watching || !node) return;
    let frame = 0;
    const publish = () => {
      frame = 0;
      const rect = node.getBoundingClientRect();
      controllerRef.current.reportBounds({ x: rect.x, y: rect.y, width: rect.width, height: rect.height });
    };
    const schedule = () => { if (frame) return; frame = requestAnimationFrame(publish); };
    publish();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    observer?.observe(node);
    window.addEventListener("resize", schedule);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", schedule);
      if (frame) cancelAnimationFrame(frame);
      controllerRef.current.clearSlotBounds();
    };
  }, [watching]);
  if (!panel) return null;
  const url = panel.phase === "opened" ? current?.url ?? panel.url : panel.url;
  const live = panel.phase === "opened" && !!current;
  const verify = current ? controller.verification[current.label] : undefined;
  const route = network.data && (network.data.content.mode === "inherit" ? network.data.global : network.data.content);
  const open = (mode: WebOpenMode) => { if (mode === "reader") controller.backToReader(url); else { onBack(); onOpenLink(url, "", mode); } };
  const confirm = async () => {
    if (never) {
      const result = await run([{ id: "web_original_tip", value: "0" }]);
      if (result.status !== "busy" && !result.mounted) return;
      if (result.status !== "done" || result.data.failure) { setError("没能保存提示设置，请重试。"); return; }
    }
    void controller.launch(url);
  };
  const choose = (action: WebViewAction | "extract") => {
    setMenu(null);
    if (action === "extract") void controller.extract();
    else void controller.act(action);
  };
  const tool = (action: WebViewAction, label: string, icon: Parameters<typeof Icon>[0]["name"]) =>
    <button type="button" className="web-tool" aria-label={label} title={label} disabled={!live} onClick={() => void controller.act(action)}><Icon name={icon} /></button>;
  if (!watching) return <>
    <WebReadingBar page={{ url }} view="web" onRead={() => open("reader")} onBack={onBack} />
    <div className="drawer-body web-window-body">
      {panel.phase === "tip" ? <div className="web-tip web-original-tip" data-tone="warn" role="note">
        <div className="web-tip-text"><strong>原网页会运行 {new URL(url).hostname} 自己的脚本。</strong>登录和验证状态只保留到这次退出 RRSS，下次启动全部清空。不要在里面输入密码、验证码或支付信息。</div>
        <div className="web-tip-actions"><label><input type="checkbox" checked={never} disabled={writing} onChange={event => setNever(event.target.checked)} />不再显示</label><button type="button" className="btn sm primary" disabled={writing} aria-busy={writing} onClick={() => void confirm()}>打开原网页</button><button type="button" className="btn sm ghost" disabled={writing} onClick={onBack}>取消</button></div>
        {error && <small role="alert">{error}</small>}
      </div> : <div className="web-tip web-original-tip web-warning" data-tone="warn" role="alert">
        <div className="web-tip-text"><strong>{panel.error?.includes("代理") ? "原网页用不了你现在的代理设置。" : "没能打开原网页。"}</strong><p>{panel.error}</p></div>
        <div className="web-tip-actions"><button type="button" className="btn primary" onClick={() => open("reader")}>改用阅读视图</button><button type="button" className="btn" onClick={() => open("browser")}>在系统浏览器打开</button><button type="button" className="btn ghost" onClick={() => { onBack(); onProxySettings(); }}>查看代理设置</button></div>
        <small>改代理设置后可再试。</small>
      </div>}
    </div>
  </>;
  return <>
    <div className="web-bar web-embed-bar" data-tauri-drag-region="deep">
      <WebReadingNav page={{ url }} onBack={onBack} live />
      <span className="web-embed-hint">原网页 · 站点脚本在运行 · <kbd>Esc</kbd> 回到阅读视图</span>
      <div className="web-mode" role="group" aria-label="看法"><button type="button" aria-pressed={false} onClick={() => open("reader")}>阅读视图</button><button type="button" aria-pressed>原网页</button></div>
      <span className="reader-head-divider" />
      {tool("back", "后退", "chevronLeft")}
      {tool("forward", "前进", "chevronRight")}
      {tool("reload", "刷新", "refresh")}
      {tool("open", "在系统浏览器打开", "external")}
      <span className="reader-head-divider" />
      <button ref={menuButton} type="button" className="web-tool" aria-label="更多" aria-haspopup="menu" aria-expanded={!!menu} disabled={!live}
        onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); setMenu(menu ? null : { x: rect.right - 248, y: rect.bottom + 4 }); }}><span aria-hidden="true">⋯</span></button>
    </div>
    {verify ? <WebVerifyStrip event={verify} busy={controller.extracting === verify.label} onAbandon={controller.abandon}
      onExtract={() => void controller.extract()} onRead={() => controller.readExtracted(verify)} /> : null}
    <div className="web-embed-slot" ref={slotRef}>
      {hidden && live ? <div className="web-embed-paused" role="status"><p>网页暂时收起</p><small>{PAUSED}</small></div>
        : panel.phase === "opening" ? <p className="web-embed-opening" role="status">正在打开原网页…</p> : null}
    </div>
    {menu ? <FloatingMenu anchor={menu} className="menu web-embed-menu" label="原网页操作" placement="below" restoreFocusTo={menuButton.current} onClose={() => setMenu(null)}>
      <div className="web-embed-menu-note">{route ? `内容网络 · ${routeSummary(route)}` : "内容网络 · 当前设置"} · 退出 RRSS 即清空</div>
      {MENU.map(([action, label, icon]) => <button key={action} type="button" role="menuitem" className="menu-item" onClick={() => choose(action)}><Icon name={icon} size={15} />{label}</button>)}
    </FloatingMenu> : null}
  </>;
}
