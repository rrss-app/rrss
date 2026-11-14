import { ruleConditionLabel } from "../lib/ruleLabels";
import { useLayoutEffect, useRef, type CSSProperties } from "react";
import type { ArticleView, FeatureView } from "../lib/api";
import { getFocusableElements, isFocusable } from "../lib/useModalFocus";
import { useTranslation } from "../i18n";
import { Icon } from "./Icon";
import { ThumbIcon } from "./ThumbIcon";

export function TrainingBar({
  item,
  onClose,
  onConfirm,
  style,
  busy = false,
  error = "",
}: {
  item: { article: ArticleView; features: FeatureView[]; dislike: boolean };
  onClose: () => void;
  onConfirm: (f: FeatureView, feedScope: boolean) => void;
  style?: CSSProperties;
  busy?: boolean;
  error?: string;
}) {
  const { t } = useTranslation("focus");
  const stage = useRef<HTMLDivElement>(null);
  const sheet = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const root = stage.current;
    const reader = document.querySelector<HTMLElement>(".drawer.reader");
    if (!root || !reader) return;
    // 仍由叠层栈管理，不挂进阅读器；仅跟随阅读区和托盘的实际边界。
    root.dataset.reader = "true";
    const tray = reader.querySelector<HTMLElement>(".reader-citation-tray");
    const measure = () => {
      const rect = reader.getBoundingClientRect();
      const bottom = tray ? tray.getBoundingClientRect().top - 12 : rect.bottom;
      root.style.setProperty("--training-left", `${rect.left}px`);
      root.style.setProperty("--training-width", `${rect.width}px`);
      root.style.setProperty("--training-bottom", `${window.innerHeight - bottom}px`);
      root.style.setProperty("--training-height", `${Math.max(0, Math.min(280, bottom - rect.top - rect.height / 2 - 8))}px`);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(reader);
    if (tray) observer.observe(tray);
    window.addEventListener("resize", measure);
    return () => { observer.disconnect(); window.removeEventListener("resize", measure); };
  }, [item.article.id]);
  useLayoutEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const panel = sheet.current;
    panel?.focus({ preventScroll: true });
    return () => {
      requestAnimationFrame(() => {
        if (!panel?.isConnected && opener?.isConnected && isFocusable(opener)) opener.focus({ preventScroll: true });
      });
    };
  }, []);
  useLayoutEffect(() => {
    if (busy) sheet.current?.focus({ preventScroll: true });
  }, [busy]);
  return (
    <div ref={stage} className="overlay bottom training-stage" style={style}>
      <button type="button" className="overlay-dismiss" aria-label={t("trainingBar.close")} disabled={busy} onClick={onClose} />
      <div ref={sheet} className="sheet training-sheet" role="dialog" aria-modal="true" aria-labelledby="training-title" aria-busy={busy} tabIndex={-1}
        onKeyDown={(event) => {
          if (event.key === "Escape" && busy) { event.preventDefault(); event.stopPropagation(); }
          if (event.key !== "Tab") return;
          const elements = getFocusableElements(event.currentTarget);
          const at = elements.indexOf(document.activeElement as HTMLElement);
          if (elements.length && (at < 0 || (event.shiftKey ? at === 0 : at === elements.length - 1))) {
            event.preventDefault();
            elements[event.shiftKey ? elements.length - 1 : 0].focus();
          } else if (!elements.length) event.preventDefault();
        }}>
        <div className="sheet-head">
          <span className={`training-kind ${item.dislike ? "down" : "up"}`}><ThumbIcon down={item.dislike} /></span>
          <div className="sheet-title">
            <strong id="training-title">{t(item.dislike ? "trainingBar.dislike" : "trainingBar.like")} · {item.article.title}</strong>
            <span className="muted">{t("trainingBar.hint")}</span>
          </div>
          <button type="button" className="tool-btn icon-only" aria-label={t("trainingBar.close")} disabled={busy} onClick={onClose}>
            <Icon name="close" size={16} />
          </button>
        </div>
        {error && <p className="notice error" role="alert">{error}</p>}
        <div className="training-list">
          {item.features.map((f) => (
            <div key={f.dimension + f.pattern} className="training-row">
              <span className="training-label">{t(item.dislike ? "trainingBar.scoreDown" : "trainingBar.scoreUp", { label: ruleConditionLabel(f, "focus") })}</span>
              <div className="row">
                <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={() => onConfirm(f, false)}>
                  {t("trainingBar.global")}
                </button>
                <button type="button" className="btn sm" disabled={busy} aria-busy={busy} onClick={() => onConfirm(f, true)}>
                  {t("trainingBar.feedOnly")}
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
