import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import type { DigestSourceView } from "../bindings";
import { useExposureTracker, type ExposureObserver } from "../lib/useExposureTracker";
import { i18n, locale, useTranslation } from "../i18n";

const CARD_ID = "digest-source-card";
const HIDE_DELAY_MS = 120;
const CARD_WIDTH = 320;

type Preview = { source: DigestSourceView; anchor: HTMLElement };

type PreviewApi = {
  anchor: HTMLElement | null;
  show: (source: DigestSourceView, anchor: HTMLElement) => void;
  hideSoon: () => void;
  close: () => void;
};

const PreviewContext = createContext<PreviewApi | null>(null);
const ExposureContext = createContext<{ observe: ExposureObserver; positionOf: (id: number) => number } | null>(null);

function SourceExposureProvider({ scrollRoot, active, sources, children }: {
  scrollRoot: RefObject<HTMLElement | null>;
  active: boolean;
  sources: DigestSourceView[];
  children: ReactNode;
}) {
  const observe = useExposureTracker({ surface: "digest", root: scrollRoot, active });
  return <ExposureContext.Provider value={{ observe, positionOf: (id) => sources.findIndex(source => source.article_id === id) }}>{children}</ExposureContext.Provider>;
}

export function sourceStatusNote(status: string): string {
  if (status === "removed") return i18n.t("source.removed", { ns: "digest" });
  if (status === "unavailable") return i18n.t("source.unavailable", { ns: "digest" });
  return "";
}

export function sourceLabel(source: DigestSourceView): string {
  const feed = source.feed_title || i18n.t("source.unknownFeed", { ns: "digest" });
  const note = sourceStatusNote(source.status);
  if (source.number != null) {
    return note
      ? i18n.t("source.labelNumberedNote", { ns: "digest", number: source.number, title: source.title, feed, note })
      : i18n.t("source.labelNumbered", { ns: "digest", number: source.number, title: source.title, feed });
  }
  return note
    ? i18n.t("source.labelNote", { ns: "digest", title: source.title, feed, note })
    : i18n.t("source.label", { ns: "digest", title: source.title, feed });
}

function formatDay(ms: number): string {
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString(locale(), { year: "numeric", month: "long", day: "numeric" });
}

/** 一篇简报内同时最多一张来源卡片；按钮与卡片视为同一悬停区域。 */
export function DigestSourcePreviewProvider({ active = true, exposureActive = active, scrollRoot, sources = [], children }: {
  /** 为 false 时关闭卡片（所在主栏目被隐藏），同时撤掉它的 Escape / 滚动监听。 */
  active?: boolean;
  exposureActive?: boolean;
  scrollRoot?: RefObject<HTMLElement | null>;
  sources?: DigestSourceView[];
  children: ReactNode;
}) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [style, setStyle] = useState<CSSProperties>({ visibility: "hidden" });
  const timer = useRef<number | undefined>(undefined);
  const cardRef = useRef<HTMLDivElement>(null);

  const cancel = () => {
    window.clearTimeout(timer.current);
    timer.current = undefined;
  };
  const close = useCallback(() => {
    cancel();
    setPreview(null);
  }, []);
  const show = useCallback((source: DigestSourceView, anchor: HTMLElement) => {
    cancel();
    setPreview((current) => (current?.anchor === anchor && current.source === source ? current : { source, anchor }));
  }, []);
  const hideSoon = useCallback(() => {
    cancel();
    timer.current = window.setTimeout(() => setPreview(null), HIDE_DELAY_MS);
  }, []);

  useEffect(() => cancel, []);
  useEffect(() => { if (!active) close(); }, [active, close]);

  useLayoutEffect(() => {
    if (!preview) return;
    const rect = preview.anchor.getBoundingClientRect();
    const height = cardRef.current?.offsetHeight ?? 0;
    const width = Math.min(CARD_WIDTH, window.innerWidth - 16);
    const below = window.innerHeight - rect.bottom - 8;
    const flip = below < height && rect.top - 8 > below;
    setStyle({
      width,
      left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)),
      top: flip ? Math.max(8, rect.top - height - 6) : rect.bottom + 6,
    });
  }, [preview]);

  useEffect(() => {
    if (!preview) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopImmediatePropagation();
      close();
    };
    const onSelection = () => {
      const selection = window.getSelection();
      if (selection && !selection.isCollapsed && selection.toString().trim()) close();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    document.addEventListener("selectionchange", onSelection);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
      document.removeEventListener("selectionchange", onSelection);
    };
  }, [preview, close]);

  const api: PreviewApi = { anchor: preview?.anchor ?? null, show, hideSoon, close };
  const source = preview?.source;
  const note = source ? sourceStatusNote(source.status) : "";
  const day = source?.published_at != null ? formatDay(source.published_at) : "";

  return (
    <PreviewContext.Provider value={api}>
      {scrollRoot ? <SourceExposureProvider scrollRoot={scrollRoot} active={exposureActive} sources={sources}>{children}</SourceExposureProvider> : children}
      {source &&
        createPortal(
          <div
            ref={cardRef}
            id={CARD_ID}
            role="tooltip"
            className="digest-source-card"
            style={style}
            onPointerEnter={cancel}
            onPointerLeave={hideSoon}
          >
            <div className="digest-source-card-meta">
              <span>{source.feed_title || i18n.t("source.unknownFeed", { ns: "digest" })}</span>
              {day ? <span>{day}</span> : null}
            </div>
            <p className="digest-source-card-title">{source.title}</p>
            {note ? <p className="digest-source-card-note">{note}</p> : source.summary ? <p className="digest-source-card-summary">{source.summary}</p> : null}
          </div>,
          document.body,
        )}
    </PreviewContext.Provider>
  );
}

function SourceButton({ source, onOpenArticle, children, className }: {
  source: DigestSourceView;
  onOpenArticle?: (id: number) => void;
  children: ReactNode;
  className: string;
}) {
  const preview = useContext(PreviewContext);
  const ref = useRef<HTMLButtonElement>(null);
  const available = source.status === "available";
  const describedBy = preview?.anchor && preview.anchor === ref.current ? CARD_ID : undefined;
  return (
    <button
      ref={ref}
      type="button"
      data-status={source.status}
      className={className}
      aria-label={sourceLabel(source)}
      aria-disabled={!available || !onOpenArticle}
      aria-describedby={describedBy}
      onPointerEnter={(e) => preview?.show(source, e.currentTarget)}
      onPointerLeave={() => preview?.hideSoon()}
      onFocus={(e) => preview?.show(source, e.currentTarget)}
      onBlur={() => preview?.hideSoon()}
      onMouseDown={(e) => e.preventDefault()}
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        if (!available || !onOpenArticle) return;
        preview?.close();
        onOpenArticle(source.article_id);
      }}
    >
      {children}
    </button>
  );
}

/** 条目末尾的来源号；只显示后端已分配来源号的来源。 */
export function DigestSourceMarks({ sources, onOpenArticle }: {
  sources: DigestSourceView[];
  onOpenArticle?: (id: number) => void;
}) {
  const numbered = sources.filter((source) => source.article_id > 0 && source.number != null);
  if (numbered.length === 0) return null;
  return (
    <span className="digest-refs" data-selection-ignore>
      {numbered.map((source) => (
        <SourceButton key={source.article_id} source={source} onOpenArticle={onOpenArticle} className="digest-ref">
          {source.number}
        </SourceButton>
      ))}
    </span>
  );
}

function SourceRow({ source, onOpenArticle }: { source: DigestSourceView; onOpenArticle: (id: number) => void }) {
  const exposure = useContext(ExposureContext);
  const observe = exposure?.observe;
  const id = source.article_id;
  const position = exposure?.positionOf(id) ?? -1;
  const ref = useCallback((element: HTMLLIElement | null) => {
    if (id > 0 && position >= 0) observe?.(element, id, position);
  }, [observe, id, position]);
  return (
    <li ref={ref}>
      {source.number != null ? <span className="digest-source-index-no" aria-hidden>{source.number}</span> : null}
      <SourceButton source={source} onOpenArticle={onOpenArticle} className="digest-source-index-link">
        <span className="digest-source-index-title">{source.title}</span>
        <span className="digest-source-index-feed">{source.feed_title || i18n.t("source.unknownFeed", { ns: "digest" })}{sourceStatusNote(source.status) ? ` · ${sourceStatusNote(source.status)}` : ""}</span>
      </SourceButton>
    </li>
  );
}

/** 文末来源表：编号与正文完全来自后端索引；未定位快照单独标明。 */
export function DigestSourceIndex({ sources, unlocated, legacy, onOpenArticle }: {
  sources: DigestSourceView[];
  unlocated: DigestSourceView[];
  legacy: boolean;
  onOpenArticle: (id: number) => void;
}) {
  const { t } = useTranslation("digest");
  const numbered = sources.filter((source) => source.number != null);
  if (numbered.length === 0 && unlocated.length === 0) return null;
  return (
    <div className="digest-source-index">
      {numbered.length > 0 ? (
        <details>
          <summary>{t("sourceIndex.numbered", { count: numbered.length })}</summary>
          <ol>{numbered.map((source) => <SourceRow key={source.article_id} source={source} onOpenArticle={onOpenArticle} />)}</ol>
        </details>
      ) : null}
      {unlocated.length > 0 ? (
        <details>
          <summary>{legacy ? t("sourceIndex.legacy", { count: unlocated.length }) : t("sourceIndex.unlocated", { count: unlocated.length })}</summary>
          <p className="digest-source-index-hint muted">{t("sourceIndex.hint")}</p>
          <ul>{unlocated.map((source) => <SourceRow key={source.article_id} source={{ ...source, number: null }} onOpenArticle={onOpenArticle} />)}</ul>
        </details>
      ) : null}
    </div>
  );
}
