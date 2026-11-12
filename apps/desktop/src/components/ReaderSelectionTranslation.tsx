import { useEffect, useMemo, useState } from "react";
import { SELECTION_TRANSLATE_MAX_CHARS, type SelectionTranslation, type SelectionTranslationConfig, type TranslationCommandError } from "../bindings";
import { api, copyText } from "../lib/api";
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
  if (!config) throw { kind: "configuration", message: "翻译服务还没配置" } satisfies TranslationCommandError;
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
        setError(reason && typeof reason === "object" && "kind" in reason && "message" in reason
          ? reason as TranslationCommandError
          : { kind: "network", message: "翻译服务连接失败，请稍后再试" });
      },
    );
    return () => { active = false; };
  }, [selection, tooLong]);
  const loading = !tooLong && !result && !error;
  return <FloatingMenu anchor={anchor} boundary={selection.boundary} placement="below" role="dialog"
    label="划词翻译" className="reader-selection-translation" onClose={onClose} restoreFocusTo={selection.opener}>
    <div className="reader-selection-translation-head">
      <span><Icon name="translate" size={14} />译文</span>
      {config ? <span className="reader-selection-service">{config.service}</span> : null}
      <button type="button" aria-label="关闭划词翻译" onClick={onClose}><Icon name="close" size={14} /></button>
    </div>
    <div className="reader-selection-translation-content" aria-live="polite" aria-busy={loading}>
      {loading ? <div className="reader-selection-skeleton" role="status" aria-label="翻译中"><i /><i /></div> : null}
      {result ? <p className="reader-selection-result">{result.text}</p> : null}
      {tooLong ? <p role="alert">选中文字超过 {SELECTION_TRANSLATE_MAX_CHARS} 字，请改用全文翻译。</p> : null}
      {error ? <p role="alert">{error.kind === "configuration" && !config ? "翻译服务还没配置。" : error.message}
        {error.kind === "configuration" || error.kind === "authentication" ? <button type="button" className="reader-selection-settings" onClick={() => { onClose(); onSettings(); }}>去设置</button> : null}
      </p> : null}
    </div>
    <div className="reader-selection-translation-footer">
      <div className="reader-selection-translation-actions">
        <button type="button" disabled={!result} onClick={() => { if (result) void copyText(result.text); }}><Icon name="copy" size={14} />复制译文</button>
        <button type="button" onClick={() => { onClose(); window.getSelection()?.removeAllRanges(); onCite(selection.excerpt, selection.blockId); }}><Icon name="quote" size={14} />引用原句</button>
      </div>
      <small className="reader-selection-note">只翻这一段，不写入全文译文</small>
    </div>
  </FloatingMenu>;
}
