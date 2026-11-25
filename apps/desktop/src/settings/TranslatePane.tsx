import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { formatNotice } from "../lib/notice";
import { aiMutationOptions } from "./aiQuery";
import { ChipBar, Field, SavedSecretField, SaveBar, SettingRow, Toggle } from "./chips";
import { readGate, useMergedEditState, useSettingsWrite, type SettingsEditState } from "./editState";
import { findEntry, ModelMenu, modelName, taskChoice, usePoolChoices } from "./modelChoices";
import { TRANSLATE_BACKENDS, translateError } from "./translateQuery";
import { useTranslateSettings } from "./useTranslateSettings";
import { SettingsRows } from "./SettingsRows";
import { titleTranslations } from "../lib/titleTranslations";
import { Pending } from "../components/Pending";
import { locale, Trans, useTranslation } from "../i18n";

/** `auto` 跟随界面语言（默认），其余是常用的目标语言；别的语言在「其他语言」里填代码。 */
const LANG_IDS = ["auto", "zh", "en", "ja"] as const;

export function TranslatePane({ onEditStateChange, onOpenModels, onChanged }: {
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onOpenModels: () => void;
  /** 自动翻译的开关会改变列表里的标题，列表据此重查。 */
  onChanged?: () => void;
}) {
  const { t } = useTranslation("settingsJobs");
  // 跟随界面时实际用的语言，与后端 `ui_language::translation_target` 一致。
  const followed = locale().startsWith("zh") ? "zh" : "en";
  const LANGS = LANG_IDS.map((id) => ({ id, label: id === "auto" ? t("translate.lang.auto", { current: t(`translate.lang.${followed}`) }) : t(`translate.lang.${id}`) }));
  const { report } = useMergedEditState(onEditStateChange);
  const { query, draft, baseline, dirty, busy, pending, status, change, pick, run, reload, discard } = useTranslateSettings(report("service"));
  // 服务、目标语言或总开关变了：列表标题要按新配置重新判断，之前失败或暂停的也重新请求。
  const identity = baseline ? `${baseline.enabled}:${baseline.backend}:${baseline.target_lang}` : "";
  useEffect(() => { if (identity) titleTranslations.reset(); }, [identity]);
  const autoChanged = () => { titleTranslations.reset(); onChanged?.(); };
  if (!draft || !baseline) return (
    <div className="translate-pane">
      {query.isError ? <>
        <p className="notice error" role="alert">{translateError(t("translate.notice.readFailedRetry"), query.error)}</p>
        <button type="button" className="btn" disabled={query.isFetching || busy} onClick={reload}>{t("translate.retry")}</button>
      </> : <Pending>{t("translate.loading")}</Pending>}
    </div>
  );

  return (
    <div className="translate-pane">
      {query.isError && !status && <p className="notice error" role="alert">{translateError(t("translate.notice.readFailedKept"), query.error, draft)}</p>}
      <fieldset className="settings-fields" disabled={busy} aria-busy={busy}>
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>{t("translate.reading.title")}</h4>
          </div>
          <div className="settings-section-body">
            <SettingRow label={t("translate.reading.enable")} hint={t("translate.reading.enableHint")}>
              <Toggle label={t("translate.reading.enable")} on={baseline.enabled} onLabel="" offLabel=""
                onClick={() => pick("enabled", !baseline.enabled)} />
            </SettingRow>
            <SettingRow label={t("translate.reading.targetLang")}>
              <ChipBar value={LANGS.some((l) => l.id === draft.target_lang) ? draft.target_lang : ""}
                className="segmented" options={LANGS} onChange={(value) => pick("target_lang", value)} />
            </SettingRow>
            {/* 已存的是常用三种以外的语言时默认展开，不让当前值藏起来。 */}
            <details className="settings-more" open={!LANGS.some((l) => l.id === baseline.target_lang) || undefined}><summary>{t("translate.reading.otherLangs")}</summary>
              <Field label={t("translate.reading.langCode")} hint={t("translate.reading.langCodeHint")}>
                <input value={draft.target_lang} onChange={(e) => change("target_lang", e.target.value)} placeholder="zh" aria-label={t("translate.reading.langCode")} />
              </Field>
            </details>
          </div>
        </section>
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>{t("translate.auto.title")}</h4>
            <p className="hint">{t("translate.auto.hint")}</p>
          </div>
          <div className="settings-section-body">
            {baseline.enabled ? null : <p className="hint">{t("translate.auto.needsReading")}</p>}
            <SettingsRows ids={["translate_auto_titles", "translate_auto_body", "translate_auto_view", "translate_auto_max_words", "translate_auto_scope"]}
              label={t("translate.auto.title")} disabled={busy || !baseline.enabled} onEditStateChange={report("auto")} onChanged={autoChanged} />
          </div>
        </section>
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>{t("translate.service.title")}</h4>
            <p className="hint">{t("translate.service.hint")}</p>
          </div>
          <div className="settings-section-body">
            <div role="group" aria-label={t("translate.service.aria")}>
              <ChipBar className="segmented" value={baseline.backend} options={TRANSLATE_BACKENDS} onChange={(value) => pick("backend", value)} />
            </div>
            {baseline.backend === "complete" ? (<>
              <TranslateModel disabled={busy} onOpenModels={onOpenModels} />
              {/* 每次请求的原文字数只对模型翻译生效；改了不使已有译文失效，下一篇开始用。 */}
              <SettingsRows ids={["translate_window_chars"]} label={t("translate.service.title")} disabled={busy}
                onEditStateChange={report("window")} />
            </>) : baseline.backend === "deepl" ? (
              <div>
                <p className="hint">{t("translate.deepl.hint")}</p>
                <SavedSecretField saved={query.data?.deepl_key ?? { state: "none", hint: "" }} label={t("translate.deepl.keyLabel")} value={draft.deepl_key} onChange={(value) => change("deepl_key", value)} placeholder={t("translate.deepl.keyPlaceholder")} />
                <Field label={t("translate.deepl.urlLabel")}>
                  <input value={draft.deepl_url} aria-label={t("translate.deepl.urlLabel")}
                    onChange={(e) => change("deepl_url", e.target.value)} placeholder={t("translate.deepl.urlPlaceholder")} />
                </Field>
              </div>
            ) : baseline.backend === "deeplx" ? (
              <div>
                <p className="hint">{t("translate.deeplx.hint")}</p>
                <Field label={t("translate.deeplx.urlLabel")}>
                  <input value={draft.deeplx_url} aria-label={t("translate.deeplx.urlLabel")}
                    onChange={(e) => change("deeplx_url", e.target.value)} placeholder="http://127.0.0.1:1188" />
                </Field>
                <SavedSecretField saved={query.data?.deeplx_token ?? { state: "none", hint: "" }} label={t("translate.deeplx.tokenLabel")} value={draft.deeplx_token} onChange={(value) => change("deeplx_token", value)} placeholder="Authorization Bearer Token" />
              </div>
            ) : (
              <div>
                <p className="hint">{t("translate.http.hint")}</p>
                <Field label={t("translate.http.urlLabel")}>
                  <input value={draft.http_url} aria-label={t("translate.http.urlLabel")}
                    onChange={(e) => change("http_url", e.target.value)} placeholder="http://127.0.0.1:5000" />
                </Field>
                <SavedSecretField saved={query.data?.http_key ?? { state: "none", hint: "" }} label={t("translate.http.keyLabel")} value={draft.http_key} onChange={(value) => change("http_key", value)} placeholder={t("translate.http.keyPlaceholder")} />
              </div>
            )}
            <div className="row form-actions translate-actions">
              <button type="button" className="btn" aria-busy={pending === "test" || undefined} title={dirty ? t("translate.service.testDirtyTitle") : undefined}
                onClick={() => void run("test")}>{t("translate.service.test")}</button>
              {status ? <span className={`form-status ${status.tone === "err" ? "error" : ""}`} role={status.tone === "err" ? "alert" : "status"}>{status.text}</span> : null}
            </div>
          </div>
        </section>
        {dirty && <SaveBar label={t("translate.editLabel")} busy={busy} saving={pending === "save"} onSave={() => void run("save")} onUndo={discard} />}
      </fieldset>
    </div>
  );
}

/** 「大模型」服务下就地选翻译用途的模型，与模型页的翻译行同一用途、同一查询键，选即存。 */
function TranslateModel({ disabled, onOpenModels }: { disabled: boolean; onOpenModels: () => void }) {
  const { t } = useTranslation("settingsJobs");
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const { pool, providers, bindings } = usePoolChoices(readGate(writing));
  const [note, setNote] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const value = taskChoice(bindings, "article_translate");
  return (
    <SettingRow label={t("translate.model.label")} hint={t("translate.model.hint")}>
      <div className="row translate-model">
        <ModelMenu label={t("translate.model.menuLabel")} value={value} providers={providers} pool={pool} disabled={disabled || writing || !bindings}
          onManage={onOpenModels} onPick={async (choice) => {
            setNote(null);
            const outcome = await run({ type: "bindings", args: { bindings: [{ task: "article_translate", ...choice }] } });
            if (outcome.status === "busy" || !outcome.mounted) return;
            const e = findEntry(pool, choice);
            setNote(outcome.status === "failed"
              ? { tone: "err", text: formatNotice(t("notice.saveFailed"), outcome.error) }
              : { tone: "ok", text: t("translate.model.saved", { model: e ? modelName(e) : choice.model }) });
          }} />
        {note && <span className={note.tone === "err" ? "form-status error" : "form-status"} role={note.tone === "err" ? "alert" : "status"}>{note.text}</span>}
      </div>
    </SettingRow>
  );
}
