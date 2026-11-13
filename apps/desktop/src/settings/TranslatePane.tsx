import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { formatNotice } from "../lib/notice";
import { aiMutationOptions } from "./aiQuery";
import { ChipBar, Field, SavedSecretField, SaveBar, SettingRow, Toggle } from "./chips";
import { readGate, useSettingsWrite, type SettingsEditState } from "./editState";
import { findEntry, ModelMenu, modelName, taskChoice, usePoolChoices } from "./modelChoices";
import { TRANSLATE_BACKENDS, translateError } from "./translateQuery";
import { useTranslateSettings } from "./useTranslateSettings";
import { Pending } from "../components/Pending";
import { Trans, useTranslation } from "../i18n";

const LANG_IDS = ["zh", "en", "ja"] as const;

export function TranslatePane({ onEditStateChange, onOpenModels }: {
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onOpenModels: () => void;
}) {
  const { t } = useTranslation("settingsJobs");
  const LANGS = LANG_IDS.map((id) => ({ id, label: t(`translate.lang.${id}`) }));
  const { query, draft, baseline, dirty, busy, pending, status, change, pick, run, reload, discard } = useTranslateSettings(onEditStateChange);
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
            <h4>{t("translate.service.title")}</h4>
            <p className="hint">{t("translate.service.hint")}</p>
          </div>
          <div className="settings-section-body">
            <div role="group" aria-label={t("translate.service.aria")}>
              <ChipBar className="segmented" value={baseline.backend} options={TRANSLATE_BACKENDS} onChange={(value) => pick("backend", value)} />
            </div>
            {baseline.backend === "complete" ? (
              <TranslateModel disabled={busy} onOpenModels={onOpenModels} />
            ) : baseline.backend === "deepl" ? (
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

/** 「大模型」服务下就地选翻译用途的模型（阶段 19-7），与模型页的翻译行同一用途、同一查询键，选即存。 */
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
