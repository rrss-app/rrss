import { useEffect, useMemo, useState } from "react";
import { SELECTION_TRANSLATE_MAX_CHARS, type SelectionTranslation, type SelectionTranslationConfig, type TranslationCommandError } from "../bindings";
import { formatError } from "../lib/errors";
import { api, copyText } from "../lib/api";
import { i18n, useTranslation } from "../i18n";
import type { ReaderSelection } from "./BlockView";
import { FloatingMenu } from "./FloatingMenu";
import { Icon } from "./Icon";

/** 本次运行内记住的成功结果上限。超出时丢掉最早的已完成项，不丢掉进行中的请求。 */
export const SELECTION_TRANSLATION_LIMIT = 200;

type CachedSelection = { promise: Promise<SelectionTranslation>; settled: boolean };
const translations = new Map<string, CachedSelection>();

function evictSelectionTranslations() {
  while (translations.size > SELECTION_TRANSLATION_LIMIT) {
    const oldest = [...translations.entries()].find(([, entry]) => entry.settled);
    if (!oldest) return;
    translations.delete(oldest[0]);
  }
}

export function rememberSelectionTranslation(key: string, start: () => Promise<SelectionTranslation>) {
  const existing = translations.get(key);
  if (existing) return existing.promise;
  const request = start();
  const entry: CachedSelection = { promise: request, settled: false };
  translations.set(key, entry);
  evictSelectionTranslations();
  void request.then(
    () => {
      entry.settled = true;
      evictSelectionTranslations();
    },
    () => {
      if (translations.get(key) === entry) translations.delete(key);
    },
  );
  return request;
}

export function selectionTranslationCount() {
  return translations.size;
}

export function hasSelectionTranslation(key: string) {
  return translations.has(key);
}

export function resetSelectionTranslations() {
  translations.clear();
}

async function translate(text: string, configured: (config: SelectionTranslationConfig) => void) {
  // 每次点击只读当前身份，不依赖设置页是否挂载、预载是否过期或写入从哪里发生。
  const { selection_config: config } = await api.translatePage();
  if (!config) throw { kind: "configuration" } satisfies TranslationCommandError;
  configured(config);
  const key = JSON.stringify([config.identity, text]);
  return rememberSelectionTranslation(key, () => api.translateSelection(text, config.identity));
}

export function ReaderSelectionTranslation({ selection, onClose, onCite, onSettings }: {
  selection: ReaderSelection;
  onClose: () => void;
  onCite: (excerpt: string, blockId: number) => void;
  onSettings: () => void;
}) {
  const { t } = useTranslation("reader");
  const tooLong = [...selection.excerpt].length > SELECTION_TRANSLATE_MAX_CHARS;
  const [config, setConfig] = useState<SelectionTranslationConfig | null>(null);
  const [result, setResult] = useState<SelectionTranslation | null>(null);
  const [error, setError] = useState<TranslationCommandError | null>(null);
  const anchor = useMemo(() => ({ x: selection.selection.left, y: selection.selection.bottom, selection: selection.selection }), [selection]);
  useEffect(() => {
    if (tooLong) return;
    let active = true;
    void translate(selection.excerpt, value => { if (active) setConfig(value); }).then(
      value => { if (active) setResult(value); },
      reason => {
        if (!active) return;
        setError(reason && typeof reason === "object" && "kind" in reason
          ? reason as TranslationCommandError
          : { kind: "network" });
      },
    );
    return () => { active = false; };
  }, [selection, tooLong]);
  const loading = !tooLong && !result && !error;
  return <FloatingMenu anchor={anchor} boundary={selection.boundary} placement="below" role="dialog"
    label={t("selection.label")} className="reader-selection-translation" onClose={onClose} restoreFocusTo={selection.opener}>
    <div className="reader-selection-translation-head">
      <span><Icon name="translate" size={14} />{t("selection.head")}</span>
      {config ? <span className="reader-selection-service">{config.service_code === "http" ? t("selection.httpService") : config.service}</span> : null}
      <button type="button" aria-label={t("selection.close")} onClick={onClose}><Icon name="close" size={14} /></button>
    </div>
    <div className="reader-selection-translation-content" aria-live="polite" aria-busy={loading}>
      {loading ? <div className="reader-selection-skeleton" role="status" aria-label={t("selection.translating")}><i /><i /></div> : null}
      {result ? <p className="reader-selection-result">{result.text}</p> : null}
      {tooLong ? <p role="alert">{t("selection.tooLong", { max: SELECTION_TRANSLATE_MAX_CHARS })}</p> : null}
      {error ? <p role="alert">{error.kind === "configuration" && !config ? t("selection.notConfiguredNotice") : error.error ? formatError(error.error) : t("selection.networkFailed")}
        {error.kind === "configuration" || error.kind === "authentication" ? <button type="button" className="reader-selection-settings" onClick={() => { onClose(); onSettings(); }}>{t("selection.goSettings")}</button> : null}
      </p> : null}
    </div>
    <div className="reader-selection-translation-footer">
      <div className="reader-selection-translation-actions">
        <button type="button" disabled={!result} onClick={() => { if (result) void copyText(result.text); }}><Icon name="copy" size={14} />{t("selection.copy")}</button>
        <button type="button" onClick={() => { onClose(); window.getSelection()?.removeAllRanges(); onCite(selection.excerpt, selection.blockId); }}><Icon name="quote" size={14} />{t("selection.citeOriginal")}</button>
      </div>
      <small className="reader-selection-note">{t("selection.note")}</small>
    </div>
  </FloatingMenu>;
}
