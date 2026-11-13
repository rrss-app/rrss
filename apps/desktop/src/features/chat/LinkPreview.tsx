import { useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { api, copyText, isSafeHttpUrl, type ArticlePreviewView, type CitationView } from "../../lib/api";
import { useLingering } from "../../lib/motion";
import { WebLinksContext } from "../../lib/webLinks";
import { sourceStatusNote } from "../../components/DigestSourcePreview";
import { localArticleId } from "./ChatMessages";
import { locale } from "../../i18n";

/** 停多久才弹：擦过不弹。 */
const SHOW_MS = 350;
/** 移开多久才收：移到卡片上不收。 */
const HIDE_MS = 160;
/** 预览缓存多久算新：已读状态会变，过期再取。 */
const FRESH_MS = 30_000;
const CARD_WIDTH = 320;
/** 只在对话与简报追问的消息、输入区的待发引用里生效；简报正文的来源卡另有一套。 */
const SCOPE = ".bubble, .composer";

type Target =
  | { kind: "local"; id: number; label: string; anchor: HTMLElement }
  | { kind: "external"; href: string; site: string; anchor: HTMLElement };

export function siteOf(href: string): string {
  try {
    return new URL(href).hostname.replace(/^www\./, "");
  } catch {
    return "";
  }
}

function targetOf(node: EventTarget | null): Target | null {
  if (!(node instanceof Element)) return null;
  const scope = node.closest(SCOPE);
  if (!scope) return null;
  const link = node.closest<HTMLAnchorElement>("a[href]");
  if (link && scope.contains(link)) {
    const href = link.getAttribute("href") ?? "";
    const id = localArticleId(href);
    if (id !== null) return { kind: "local", id, label: link.textContent ?? "", anchor: link };
    return isSafeHttpUrl(href) ? { kind: "external", href, site: siteOf(href), anchor: link } : null;
  }
  const chip = node.closest<HTMLElement>(".citation-chip[data-article-id]");
  const id = Number(chip?.dataset.articleId);
  return chip && scope.contains(chip) && Number.isSafeInteger(id) && id > 0
    ? { kind: "local", id, label: chip.title, anchor: chip } : null;
}

/** 同一条消息（或输入区）里所有本地文章 id：悬停其中一个时一次取齐。 */
function siblingIds(anchor: HTMLElement): number[] {
  const scope = anchor.closest(SCOPE);
  if (!scope) return [];
  const ids = new Set<number>();
  scope.querySelectorAll<HTMLElement>("a[href^='rrss://article/'], .citation-chip[data-article-id]").forEach((node) => {
    const id = node instanceof HTMLAnchorElement ? localArticleId(node.getAttribute("href") ?? "") : Number(node.dataset.articleId);
    if (id && Number.isSafeInteger(id) && id > 0) ids.add(id);
  });
  return [...ids];
}

/** 本次运行的预览缓存：按 id 合批取，过期重取；取不到时保留旧值。 */
function usePreviews() {
  const cache = useRef(new Map<number, { view: ArticlePreviewView; at: number }>());
  const [, setRevision] = useState(0);
  const load = useCallback((ids: number[]) => {
    const now = Date.now();
    const stale = ids.filter((id) => (cache.current.get(id)?.at ?? 0) < now - FRESH_MS);
    if (!stale.length) return;
    void api.articlePreviews(stale).then((views) => {
      const at = Date.now();
      for (const view of views) cache.current.set(view.article_id, { view, at });
      setRevision((value) => value + 1);
    }).catch(() => {});
  }, []);
  const get = useCallback((id: number) => cache.current.get(id)?.view ?? null, []);
  return useMemo(() => ({ get, load }), [get, load]);
}

function formatDay(ms: number | null): string {
  if (ms == null) return "";
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString(locale(), { month: "long", day: "numeric" });
}

export type LinkPreviewProps = {
  onOpenArticle: (id: number) => void;
  /** 「引用到输入框」：加到当前输入区的待发引用。 */
  onCite: (citation: CitationView) => void;
  notify: (notice: { message: string }) => void;
};

/**
 * 对话里的链接与引用小签悬停预览（设计系统「对话页打磨 · 本地来源预览」）：停 350ms 弹出 320px 卡片，
 * 已开时移到另一处立即换内容；移开 160ms 收起，移到卡片上不收；滚动、按下或右键时收起；优先显示在下方。
 * 键盘移到链接上同样弹出；程序归还焦点、内容滑到静止的指针下不弹。
 * 本地文章显示来源、日期、已读、标题与摘要；外部链接只显示地址与站点，不发任何请求。
 */
export function LinkPreview({ onOpenArticle, onCite, notify }: LinkPreviewProps) {
  const [target, setTarget] = useState<Target | null>(null);
  const shown = useLingering(target, 100);
  const [style, setStyle] = useState<CSSProperties>({ visibility: "hidden" });
  const [above, setAbove] = useState(false);
  const card = useRef<HTMLDivElement>(null);
  const timer = useRef<number | undefined>(undefined);
  const current = useRef<Target | null>(null);
  current.current = target;
  const previews = usePreviews();
  const webLinks = useContext(WebLinksContext);

  const cancel = () => { window.clearTimeout(timer.current); timer.current = undefined; };
  const close = useCallback(() => { window.clearTimeout(timer.current); setTarget(null); }, []);
  const show = useCallback((next: Target) => {
    window.clearTimeout(timer.current);
    if (next.kind === "local") previews.load(siblingIds(next.anchor).concat(next.id));
    setTarget(next);
  }, [previews]);
  const hideSoon = useCallback(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setTarget(null), HIDE_MS);
  }, []);

  useEffect(() => {
    let pending: HTMLElement | null = null;
    const arm = (next: Target) => {
      const open = current.current;
      if (open?.anchor === next.anchor) { cancel(); return; }
      if (!open && pending === next.anchor) return;
      window.clearTimeout(timer.current);
      pending = next.anchor;
      // 卡片已开：立即换成这一处；没开：停够了才弹。
      if (open) show(next);
      else timer.current = window.setTimeout(() => { pending = null; show(next); }, SHOW_MS);
    };
    // 只认真的指针移动：菜单收起、内容重排时滑到静止指针下的链接不弹。
    const move = (event: PointerEvent) => {
      if (event.pointerType === "touch") return;
      const next = targetOf(event.target);
      if (next) arm(next);
      else if (pending && !(event.target instanceof Node && pending.contains(event.target))) { pending = null; cancel(); }
    };
    // 键盘移到链接上才弹；点按、右键菜单归还焦点时不弹。
    const focus = (event: FocusEvent) => {
      const next = targetOf(event.target);
      if (next?.anchor.matches(":focus-visible")) arm(next);
    };
    const leave = (event: Event) => {
      const from = targetOf(event.target);
      if (!from) return;
      const to = (event as PointerEvent | FocusEvent).relatedTarget;
      if (to instanceof Node && (from.anchor.contains(to) || card.current?.contains(to))) return;
      if (pending === from.anchor) { pending = null; cancel(); }
      if (current.current) hideSoon();
    };
    // 按下（含右键）就收起，等待中的也作废：不和点击、菜单抢位置。
    const press = (event: Event) => {
      if (card.current?.contains(event.target as Node)) return;
      pending = null;
      if (current.current) close();
      else cancel();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !current.current) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      close();
    };
    const scroll = (event: Event) => { if (current.current && !card.current?.contains(event.target as Node)) close(); };
    document.addEventListener("pointermove", move);
    document.addEventListener("pointerout", leave);
    document.addEventListener("focusin", focus);
    document.addEventListener("focusout", leave);
    document.addEventListener("pointerdown", press, true);
    document.addEventListener("contextmenu", press, true);
    window.addEventListener("keydown", escape, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", close);
    return () => {
      window.clearTimeout(timer.current);
      document.removeEventListener("pointermove", move);
      document.removeEventListener("pointerout", leave);
      document.removeEventListener("focusin", focus);
      document.removeEventListener("focusout", leave);
      document.removeEventListener("pointerdown", press, true);
      document.removeEventListener("contextmenu", press, true);
      window.removeEventListener("keydown", escape, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", close);
    };
  }, [show, hideSoon, close]);

  // 链接本身被换掉（流式回复重绘、换对话）就收起。
  useEffect(() => {
    if (target && !target.anchor.isConnected) close();
  });

  const view = shown.value;
  const local = view?.kind === "local" ? previews.get(view.id) : null;
  useLayoutEffect(() => {
    if (!view || shown.leaving) return;
    const rect = view.anchor.getBoundingClientRect();
    const height = card.current?.offsetHeight ?? 0;
    const width = Math.min(CARD_WIDTH, window.innerWidth - 16);
    const below = window.innerHeight - rect.bottom - 8;
    const flip = below < height + 6 && rect.top - 8 > below;
    setAbove(flip);
    setStyle({
      width,
      left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)),
      top: flip ? Math.max(8, rect.top - height - 6) : rect.bottom + 6,
    });
  }, [view, local, shown.leaving]);

  if (!view) return null;
  const cite = (preview: ArticlePreviewView) => {
    onCite({ kind: "article", article_id: preview.article_id, title: preview.title, feed_title: preview.feed_title, url: preview.url, excerpt: "", block_id: -1 });
    close();
  };
  return createPortal(
    <div ref={card} className="digest-source-card chat-link-card" role="dialog" aria-label={view.kind === "local" ? "文章预览" : "链接预览"}
      style={style} data-up={above ? "" : undefined} data-leaving={shown.leaving ? "" : undefined} inert={shown.leaving}
      onPointerEnter={cancel} onPointerLeave={hideSoon} onFocus={cancel}>
      {view.kind === "local" ? <>
        <div className="digest-source-card-meta">
          <span>{local?.feed_title || (local ? "未知来源" : "")}{local?.published_at != null ? ` · ${formatDay(local.published_at)}` : ""}</span>
          {local && local.status === "available" ? (local.read ? <span>已读</span> : <span className="chat-link-unread" aria-label="未读" />) : null}
        </div>
        <p className="digest-source-card-title">{local?.title || view.label.replace(/^《|》$/g, "")}</p>
        {local && local.status !== "available" ? <p className="digest-source-card-note">{sourceStatusNote(local.status)}</p>
          : local?.summary ? <p className="digest-source-card-summary">{local.summary}</p>
          : !local ? <p className="digest-source-card-note">正在读取…</p> : null}
        <div className="chat-link-actions">
          <button type="button" onClick={() => { close(); onOpenArticle(view.id); }} disabled={local?.status === "removed"}>打开<kbd>Enter</kbd></button>
          <button type="button" onClick={() => local && cite(local)} disabled={!local || local.status !== "available"}>引用到输入框</button>
        </div>
      </> : <>
        <div className="digest-source-card-meta"><span>{view.site}</span></div>
        <p className="chat-link-url">{view.href}</p>
        <div className="chat-link-actions">
          <button type="button" onClick={() => { close(); webLinks?.open(view.href, view.anchor.textContent ?? ""); }}>在原网页打开</button>
          <button type="button" onClick={() => {
            close();
            void copyText(view.href).then(() => notify({ message: "已复制链接" }), () => notify({ message: "复制失败" }));
          }}>复制链接</button>
        </div>
      </>}
    </div>,
    document.body,
  );
}
