import type { ArticleView } from "../bindings";
import { locale, useTranslation } from "../i18n";
import { useDisplayTitle } from "../lib/titleTranslations";

type TitleRow = Pick<ArticleView, "id" | "title" | "title_lang" | "title_translation" | "body_translated">;

/** 语言码的本地名称（`en` → 英语 / English）；环境不支持时退回语言码。 */
export function languageName(code: string) {
  try {
    return new Intl.DisplayNames([locale()], { type: "language" }).of(code) ?? code;
  } catch {
    return code;
  }
}

/** 列表里要读出的标题：与 `ArticleTitle` 显示的一致。 */
export function useArticleLabel(row: TitleRow) {
  return useDisplayTitle(row).title;
}

/**
 * 列表标题。标题自动翻译对这篇生效时显示译文，前面一个小的语言码，提示点进去仍是外文；
 * 正文已有译文时语言码变色。没开或不是外文时原样显示。
 */
export function ArticleTitle({ row }: { row: TitleRow }) {
  const { t } = useTranslation("focus");
  const shown = useDisplayTitle(row);
  if (!shown.lang) return <>{row.title}</>;
  const name = languageName(shown.lang);
  return <>
    <span className="title-lang" data-done={row.body_translated || undefined}
      title={t(row.body_translated ? "card.langTranslated" : "card.langOriginal", { lang: name })}>{shown.lang.toUpperCase()}</span>
    <span lang={shown.translated ? undefined : shown.lang}
      title={shown.translated ? t("card.originalTitle", { title: row.title }) : undefined}>{shown.title}</span>
  </>;
}
