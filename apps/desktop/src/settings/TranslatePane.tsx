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

const LANGS = [
  { id: "zh", label: "中文" },
  { id: "en", label: "英语" },
  { id: "ja", label: "日语" },
];

export function TranslatePane({ onEditStateChange, onOpenModels }: {
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onOpenModels: () => void;
}) {
  const { query, draft, baseline, dirty, busy, pending, status, change, pick, run, reload, discard } = useTranslateSettings(onEditStateChange);
  if (!draft || !baseline) return (
    <div className="translate-pane">
      {query.isError ? <>
        <p className="notice error" role="alert">{translateError("读取翻译设置失败，请重试：", query.error)}</p>
        <button type="button" className="btn" disabled={query.isFetching || busy} onClick={reload}>重试读取</button>
      </> : <Pending>正在读取已保存的翻译设置…</Pending>}
    </div>
  );

  return (
    <div className="translate-pane">
      {query.isError && !status && <p className="notice error" role="alert">{translateError("读取翻译设置失败，当前输入已保留：", query.error, draft)}</p>}
      <fieldset className="settings-fields" disabled={busy} aria-busy={busy}>
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>阅读</h4>
          </div>
          <div className="settings-section-body">
            <SettingRow label="启用翻译" hint="关闭后阅读器里不出现翻译。">
              <Toggle label="启用翻译" on={baseline.enabled} onLabel="" offLabel=""
                onClick={() => pick("enabled", !baseline.enabled)} />
            </SettingRow>
            <SettingRow label="目标语言">
              <ChipBar value={LANGS.some((l) => l.id === draft.target_lang) ? draft.target_lang : ""}
                className="segmented" options={LANGS} onChange={(value) => pick("target_lang", value)} />
            </SettingRow>
            {/* 已存的是常用三种以外的语言时默认展开，不让当前值藏起来。 */}
            <details className="settings-more" open={!LANGS.some((l) => l.id === baseline.target_lang) || undefined}><summary>其他语言</summary>
              <Field label="语言代码" hint="其他语言可填写代码，例如 de（德语）、fr（法语）。">
                <input value={draft.target_lang} onChange={(e) => change("target_lang", e.target.value)} placeholder="zh" aria-label="语言代码" />
              </Field>
            </details>
          </div>
        </section>
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>服务</h4>
            <p className="hint">选一种即生效。换服务时，各自的地址和密钥都留着。</p>
          </div>
          <div className="settings-section-body">
            <div role="group" aria-label="翻译服务">
              <ChipBar className="segmented" value={baseline.backend} options={TRANSLATE_BACKENDS} onChange={(value) => pick("backend", value)} />
            </div>
            {baseline.backend === "complete" ? (
              <TranslateModel disabled={busy} onOpenModels={onOpenModels} />
            ) : baseline.backend === "deepl" ? (
              <div>
                <p className="hint">填写 DeepL API 密钥。免费版密钥以 :fx 结尾；不填写自定义地址时，会根据密钥选择官方地址。</p>
                <SavedSecretField saved={query.data?.deepl_key ?? { state: "none", hint: "" }} label="API Key" value={draft.deepl_key} onChange={(value) => change("deepl_key", value)} placeholder="例如 12345678-abcd-...:fx" />
                <Field label="自定义 API 地址（可选）">
                  <input value={draft.deepl_url} aria-label="自定义 API 地址（可选）"
                    onChange={(e) => change("deepl_url", e.target.value)} placeholder="留空使用官方默认地址" />
                </Field>
              </div>
            ) : baseline.backend === "deeplx" ? (
              <div>
                <p className="hint">填写你已部署的 DeepLX 服务地址；服务需要认证时，再填写 Token。</p>
                <Field label="DeepLX 服务地址">
                  <input value={draft.deeplx_url} aria-label="DeepLX 服务地址"
                    onChange={(e) => change("deeplx_url", e.target.value)} placeholder="http://127.0.0.1:1188" />
                </Field>
                <SavedSecretField saved={query.data?.deeplx_token ?? { state: "none", hint: "" }} label="Token（可选）" value={draft.deeplx_token} onChange={(value) => change("deeplx_token", value)} placeholder="Authorization Bearer Token" />
              </div>
            ) : (
              <div>
                <p className="hint">填写兼容 LibreTranslate 的翻译服务地址。</p>
                <Field label="HTTP 服务地址">
                  <input value={draft.http_url} aria-label="HTTP 服务地址"
                    onChange={(e) => change("http_url", e.target.value)} placeholder="http://127.0.0.1:5000" />
                </Field>
                <SavedSecretField saved={query.data?.http_key ?? { state: "none", hint: "" }} label="API Key（可选）" value={draft.http_key} onChange={(value) => change("http_key", value)} placeholder="可选 API Key" />
              </div>
            )}
            <div className="row form-actions translate-actions">
              <button type="button" className="btn" aria-busy={pending === "test" || undefined} title={dirty ? "会先保存当前修改再测试" : undefined}
                onClick={() => void run("test")}>测试连接</button>
              {status ? <span className={`form-status ${status.tone === "err" ? "error" : ""}`} role={status.tone === "err" ? "alert" : "status"}>{status.text}</span> : null}
            </div>
          </div>
        </section>
        {dirty && <SaveBar label="翻译设置" busy={busy} saving={pending === "save"} onSave={() => void run("save")} onUndo={discard} />}
      </fieldset>
    </div>
  );
}

/** 「大模型」服务下就地选翻译用途的模型（阶段 19-7），与模型页的翻译行同一用途、同一查询键，选即存。 */
function TranslateModel({ disabled, onOpenModels }: { disabled: boolean; onOpenModels: () => void }) {
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const { pool, providers, bindings } = usePoolChoices(readGate(writing));
  const [note, setNote] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const value = taskChoice(bindings, "article_translate");
  return (
    <SettingRow label="使用模型" hint="和模型页「翻译」用途是同一项。">
      <div className="row translate-model">
        <ModelMenu label="翻译的模型" value={value} providers={providers} pool={pool} disabled={disabled || writing || !bindings}
          onManage={onOpenModels} onPick={async (choice) => {
            setNote(null);
            const outcome = await run({ type: "bindings", args: { bindings: [{ task: "article_translate", ...choice }] } });
            if (outcome.status === "busy" || !outcome.mounted) return;
            const e = findEntry(pool, choice);
            setNote(outcome.status === "failed"
              ? { tone: "err", text: formatNotice("没有保存：", outcome.error) }
              : { tone: "ok", text: `已保存：翻译改用 ${e ? modelName(e) : choice.model}。` });
          }} />
        {note && <span className={note.tone === "err" ? "form-status error" : "form-status"} role={note.tone === "err" ? "alert" : "status"}>{note.text}</span>}
      </div>
    </SettingRow>
  );
}
