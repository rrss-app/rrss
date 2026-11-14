import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, onWebClosed, onWebDownloadBlocked, onWebEscape, onWebNavigated, onWebVerifyState } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { asUiError, isProxyError, type UiErrorEnvelope } from "../../lib/errors";
import { i18n, Trans, useTranslation } from "../../i18n";
import { useSettingsWrite } from "../../settings/editState";
import { settingsMutationOptions, settingsSchemaOptions } from "../../settings/settingsQuery";
import { networkProxyQueryOptions, routeSummary } from "../../settings/networkProxyQuery";
import { FloatingMenu, type MenuAnchor } from "../../components/FloatingMenu";
import { Icon } from "../../components/Icon";
import { WebReadingBar, WebReadingNav } from "./WebReading";
import { WebVerifyStrip } from "./WebVerifyStrip";
import type { WebVerifyStateEvent, WebViewBounds } from "../../bindings";
import type { WebLinkNotice, WebOpenMode } from "../../lib/webLinks";

type Panel = { url: string; phase: "tip" | "opening" | "opened" | "error"; error?: string; failure?: UiErrorEnvelope };
type Live = { label: string; openedUrl: string; url: string };
/** 宿主 `web_view_action` 的白名单动作。 */
export type WebViewAction = "back" | "forward" | "reload" | "copy" | "open" | "disable-js" | "clear" | "close";
const ACTION_DONE = new Set<WebViewAction>(["copy", "disable-js", "clear"]);
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
    void api.setWebViewVisible(live.label, want).catch(error => callbacks.current.notify({ message: formatNotice(i18n.t("webWindow.showFailedPrefix", { ns: "reader" }), error) }));
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
      onWebDownloadBlocked(({ url }) => callbacks.current.notify({ message: i18n.t("webWindow.downloadBlocked", { ns: "reader" }), actionLabel: i18n.t("webWindow.systemBrowser", { ns: "reader" }), onAction: () => callbacks.current.openLink(url, "", "browser") })),
    ];
    for (const subscription of subscriptions) void subscription.then(off => { if (disposed) off(); else unlisten.push(off); }).catch(error => { if (!disposed) callbacks.current.notify({ message: formatNotice(i18n.t("webWindow.eventsFailedPrefix", { ns: "reader" }), error) }); });
    return () => { disposed = true; ++requestGeneration.current; dropWait(); unlisten.forEach(off => off()); };
  }, [clearVerification, dropWait, receiveVerification]);
  const pushBounds = useCallback((bounds: WebViewBounds) => {
    const live = liveRef.current;
    if (!live || panelRef.current?.phase !== "opened" || closed.current.has(live.label)) return;
    if (placedBounds.current && sameBounds(placedBounds.current, bounds)) return;
    placedBounds.current = bounds;
    void api.setWebViewBounds(live.label, bounds).catch(error => callbacks.current.notify({ message: formatNotice(i18n.t("webWindow.boundsFailedPrefix", { ns: "reader" }), error) }));
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
        if (ticket === generation.current) showPanel({ url, phase: "error", error: formatNotice("", error), failure: asUiError(error) });
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
      if (ticket === generation.current) showPanel({ url, phase: "error", error: formatNotice("", error), failure: asUiError(error) });
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
      if (ticket === epoch.current && liveRef.current?.url === url) callbacks.current.notify({ message: formatNotice(i18n.t("webWindow.extractFailedPrefix", { ns: "reader" }), error) });
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
    void api.closeWebView(label).catch(error => callbacks.current.notify({ message: formatNotice(i18n.t("webWindow.closeFailedPrefix", { ns: "reader" }), error) }));
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
        if (ACTION_DONE.has(action)) callbacks.current.notify({ message: i18n.t(`webWindow.actionDone.${action as "copy" | "disable-js" | "clear"}`, { ns: "reader" }) });
      } catch (error) { callbacks.current.notify({ message: formatNotice(i18n.t("webWindow.actionFailedPrefix", { ns: "reader" }), error) }); }
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

const MENU: [Exclude<WebViewAction, "back" | "forward" | "reload"> | "extract", Parameters<typeof Icon>[0]["name"]][] = [
  ["copy", "copy"], ["open", "external"], ["extract", "fileText"],
  ["disable-js", "stop"], ["clear", "trash"], ["close", "close"],
];

export function WebWindowScene({ controller, covered, onBack, onOpenLink, onProxySettings }: {
  controller: ReturnType<typeof useWebWindows>; covered: boolean; onBack: () => void;
  onOpenLink: (url: string, text?: string, mode?: WebOpenMode) => void; onProxySettings: () => void;
}) {
  const { t } = useTranslation("reader");
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
      if (result.status !== "done" || result.data.failure) { setError(t("webWindow.tipSaveFailed")); return; }
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
        <div className="web-tip-text"><Trans t={t} i18nKey="webWindow.tip.text" values={{ host: new URL(url).hostname }} components={{ strong: <strong /> }} /></div>
        <div className="web-tip-actions"><label><input type="checkbox" checked={never} disabled={writing} onChange={event => setNever(event.target.checked)} />{t("web.firstTip.never")}</label><button type="button" className="btn sm primary" disabled={writing} aria-busy={writing} onClick={() => void confirm()}>{t("web.openOriginal")}</button><button type="button" className="btn sm ghost" disabled={writing} onClick={onBack}>{t("webWindow.tip.cancel")}</button></div>
        {error && <small role="alert">{error}</small>}
      </div> : <div className="web-tip web-original-tip web-warning" data-tone="warn" role="alert" data-error-code={panel.failure?.code}>
        <div className="web-tip-text"><strong>{isProxyError(panel.failure) ? t("webWindow.error.proxy") : t("webWindow.error.open")}</strong><p>{panel.error}</p></div>
        <div className="web-tip-actions"><button type="button" className="btn primary" onClick={() => open("reader")}>{t("webWindow.error.useReader")}</button><button type="button" className="btn" onClick={() => open("browser")}>{t("web.openInBrowser")}</button><button type="button" className="btn ghost" onClick={() => { onBack(); onProxySettings(); }}>{t("webWindow.error.proxySettings")}</button></div>
        <small>{t("webWindow.error.retryHint")}</small>
      </div>}
    </div>
  </>;
  return <>
    <div className="web-bar web-embed-bar" data-tauri-drag-region="deep">
      <WebReadingNav page={{ url }} onBack={onBack} live />
      <span className="web-embed-hint"><Trans t={t} i18nKey="webWindow.hint" components={{ kbd: <kbd /> }} /></span>
      <div className="web-mode" role="group" aria-label={t("web.viewLabel")}><button type="button" aria-pressed={false} onClick={() => open("reader")}>{t("web.readerView")}</button><button type="button" aria-pressed>{t("web.originalPage")}</button></div>
      <span className="reader-head-divider" />
      {tool("back", t("webWindow.tool.back"), "chevronLeft")}
      {tool("forward", t("webWindow.tool.forward"), "chevronRight")}
      {tool("reload", t("webWindow.tool.reload"), "refresh")}
      {tool("open", t("web.openInBrowser"), "external")}
      <span className="reader-head-divider" />
      <button ref={menuButton} type="button" className="web-tool" aria-label={t("webWindow.tool.more")} aria-haspopup="menu" aria-expanded={!!menu} disabled={!live}
        onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); setMenu(menu ? null : { x: rect.right - 248, y: rect.bottom + 4 }); }}><span aria-hidden="true">⋯</span></button>
    </div>
    {verify ? <WebVerifyStrip event={verify} busy={controller.extracting === verify.label} onAbandon={controller.abandon}
      onExtract={() => void controller.extract()} onRead={() => controller.readExtracted(verify)} /> : null}
    <div className="web-embed-slot" ref={slotRef}>
      {hidden && live ? <div className="web-embed-paused" role="status"><p>{t("webWindow.paused.title")}</p><small>{t("webWindow.paused.detail")}</small></div>
        : panel.phase === "opening" ? <p className="web-embed-opening" role="status">{t("webWindow.opening")}</p> : null}
    </div>
    {menu ? <FloatingMenu anchor={menu} className="menu web-embed-menu" label={t("webWindow.menu.label")} placement="below" restoreFocusTo={menuButton.current} onClose={() => setMenu(null)}>
      <div className="web-embed-menu-note">{route ? t("webWindow.menu.noteRoute", { route: routeSummary(route) }) : t("webWindow.menu.noteDefault")}</div>
      {MENU.map(([action, icon]) => <button key={action} type="button" role="menuitem" className="menu-item" onClick={() => choose(action)}><Icon name={icon} size={15} />{t(`webWindow.menu.${action}`)}</button>)}
    </FloatingMenu> : null}
  </>;
}
