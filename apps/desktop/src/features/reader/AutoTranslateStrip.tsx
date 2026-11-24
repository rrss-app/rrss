import type { AutoTranslateView } from "../../bindings";
import { languageName } from "../../components/ArticleTitle";
import { locale, useTranslation } from "../../i18n";

/**
 * 阅读页标题下的语言条：外文文章在自动翻译生效时说明正文的翻译状态。
 * 进度与失败沿用现有的翻译状态行，这里只说「翻没翻、为什么没翻」。
 */
export function AutoTranslateStrip({ auto, started, ready, busy, never, onTranslate, onNever, onSettings }: {
  auto: AutoTranslateView;
  /** 这次打开由自动翻译启动（或直接进入了已有译文）。 */
  started: boolean;
  ready: boolean;
  busy: boolean;
  /** 已把这个源设为不再自动翻译。 */
  never: boolean;
  onTranslate: () => void;
  onNever?: () => void;
  onSettings?: () => void;
}) {
  const { t } = useTranslation("reader");
  const lang = languageName(auto.lang);
  const count = (n: number) => new Intl.NumberFormat(locale()).format(n);
  if (never) return <p className="reader-lang-strip" role="status"><span>{t("autoTranslate.never")}</span></p>;
  if (ready || busy) {
    if (!started) return null;
    return <p className="reader-lang-strip" data-tone={ready ? "ok" : "run"} role="status">
      <span>{ready ? t("autoTranslate.done", { lang }) : t("autoTranslate.running")}</span>
      {onNever ? <button type="button" className="quiet" onClick={onNever}>{t("autoTranslate.stopFeed")}</button> : null}
    </p>;
  }
  const action = (label: string, run: () => void) => <button type="button" onClick={run}>{label}</button>;
  switch (auto.body) {
    case "off":
      return <p className="reader-lang-strip"><span>{t("autoTranslate.original", { lang })}</span>{action(t("autoTranslate.translate"), onTranslate)}</p>;
    case "too_long":
      return <p className="reader-lang-strip" data-tone="warn"><span>{t("autoTranslate.tooLong", { words: count(auto.words), max: count(auto.max_words) })}</span>{action(t("autoTranslate.translate"), onTranslate)}</p>;
    case "budget":
      return <p className="reader-lang-strip" data-tone="warn"><span>{t("autoTranslate.budget")}</span>{action(t("autoTranslate.translateAnyway"), onTranslate)}</p>;
    case "unconfigured":
      return <p className="reader-lang-strip" data-tone="warn"><span>{t("autoTranslate.unconfigured")}</span>{onSettings ? action(t("autoTranslate.settings"), onSettings) : null}</p>;
    default:
      return null;
  }
}
