import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ConnectionTestView, ProviderPreset, ProviderView } from "../lib/api";
import { Dialog } from "./Dialog";
import { aiMutationOptions, bindingsQueryOptions, poolQueryOptions, testConnection, type AiWrite } from "./aiQuery";
import { explainAiError, secretsFromConnection, type AiErrorView } from "./aiError";
import { probeWaitLabel, useProbe } from "./aiProbe";
import { ChipBar, Field, SecretField } from "./chips";
import { CompatEditor, jsonObject, prettyJson, sameJson } from "./compat";
import { useSettingsWrite, type SettingsEditState } from "./editState";
import type { Ask } from "./SettingsPanel";

type Draft = { kind: string; name: string; base_url: string; protocol: string; compat_json: string };
type Result = { tone: "ok" | "err"; text: string; detail?: string };

const PROTOCOLS = ["chat", "responses", "auto"].map((id) => ({ id, label: id }));

function draftFrom(provider: ProviderView | undefined, preset: ProviderPreset | undefined): Draft {
  if (provider) {
    return { kind: provider.kind, name: provider.name, base_url: provider.base_url, protocol: provider.protocol || "chat",
      compat_json: prettyJson(provider.compat_json) };
  }
  return { kind: preset?.id ?? "openai_compat", name: preset?.name ?? "", base_url: preset?.base_url ?? "",
    protocol: preset?.protocol ?? "chat", compat_json: prettyJson(preset?.compat_json) };
}

/** 测通结果的统一文案；错误详情去掉密钥。弹层与连接列表共用。 */
export function describeTest(test: ConnectionTestView, secrets: string[]): Result {
  if (!test.ok) {
    const view = explainAiError(test.error || "没有返回模型列表", secrets);
    return { tone: "err", text: `测试失败。${view.title}`, detail: view.detail };
  }
  return { tone: "ok", text: `连接成功 · ${test.latency_ms} ms · 返回 ${test.models} 个模型` };
}

/**
 * 添加 / 编辑连接（阶段 19-7）：服务商、名称、API Key；地址、协议、兼容设置在「高级」里。连接不再绑模型，
 * 添加成功后由模型页接着打开「添加模型」。已保存的密钥只显示首尾各 4 位，要改就重新输入（阶段 19-9）。
 */
export function ConnectionDialog({ provider, presets, onAsk, onClose, onSaved, onEditStateChange }: {
  provider?: ProviderView;
  presets: ProviderPreset[];
  onAsk: Ask;
  onClose: () => void;
  /** 保存成功；`created` 为真时模型页接着打开「添加模型」。 */
  onSaved: (id: number, message: string, created: boolean) => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const client = useQueryClient();
  const { writing: busy, run } = useSettingsWrite(aiMutationOptions(client));
  const pool = useQuery(poolQueryOptions()).data;
  const bindings = useQuery(bindingsQueryOptions()).data;
  const [initial] = useState(() => draftFrom(provider, presets[0]));
  const [draft, setDraft] = useState(initial);
  const [key, setKey] = useState("");
  const [replacingKey, setReplacingKey] = useState(!provider || provider.key !== "set");
  const [clearKey, setClearKey] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  const preset = presets.find((p) => p.id === draft.kind);
  const probe = useProbe();
  const dirty = !!key.trim() || clearKey || (Object.keys(initial) as (keyof Draft)[]).some((k) =>
    k === "compat_json" ? !sameJson(initial[k], draft[k]) : initial[k] !== draft[k]);
  const label = provider ? "编辑连接" : "添加连接";

  const change = <K extends keyof Draft>(field: K, value: Draft[K]) => {
    setDraft((old) => ({ ...old, [field]: value }));
    if (field !== "name") setResult(null);
  };

  const choosePreset = (id: string) => {
    const next = presets.find((p) => p.id === id);
    if (!next) return;
    setDraft((old) => ({ ...old, kind: next.id, base_url: next.base_url, protocol: next.protocol,
      compat_json: prettyJson(next.compat_json), name: !old.name || old.name === preset?.name ? next.name : old.name }));
    setResult(null);
  };
  const defaults = preset && { base_url: preset.base_url || draft.base_url, protocol: preset.protocol, compat_json: prettyJson(preset.compat_json) };
  const atDefaults = !defaults || (defaults.base_url === draft.base_url && defaults.protocol === draft.protocol && sameJson(defaults.compat_json, draft.compat_json));

  const secrets = () => secretsFromConnection(key, draft.base_url);
  const failure = (view: AiErrorView, prefix = ""): Result => ({ tone: "err", text: prefix + view.title, detail: view.detail });

  const test = async () => {
    if (!draft.base_url.trim()) { setResult({ tone: "err", text: "请先在「高级」里填写连接地址。" }); return; }
    setResult(null);
    const keys = secrets();
    try {
      const outcome = await probe.run("test", () => testConnection(client, {
        provider_id: provider?.id ?? 0, base_url: draft.base_url.trim(), api_key: clearKey ? undefined : key.trim() || undefined,
      }));
      if (!outcome.done) { if (outcome.note) setResult({ tone: "err", text: outcome.note }); return; }
      setResult(describeTest(outcome.value, keys));
    } catch (err) {
      setResult(failure(explainAiError(err, keys), "测试失败。"));
    }
  };

  /** 写入完成后关闭弹层；失败留在弹层里提示，草稿不动。 */
  const submit = async (next: AiWrite, failedPrefix: string, done: (data: { savedId: number; reread: unknown }) => void) => {
    const keys = secrets();
    const outcome = await run(next);
    if (outcome.status === "busy") { setResult({ tone: "err", text: "上一次修改还在保存，请稍候再试。" }); return false; }
    if (outcome.status === "failed") { setResult(failure(explainAiError(outcome.error, keys), failedPrefix)); return false; }
    done(outcome.data);
    return true;
  };

  const save = async () => {
    if (busy) return false;
    if (!draft.base_url.trim()) { setResult({ tone: "err", text: "请先在「高级」里填写连接地址。" }); return false; }
    if (jsonObject(draft.compat_json) === null) { setResult({ tone: "err", text: "兼容设置不是有效的 JSON 对象，未保存。" }); return false; }
    probe.stop();
    setResult(null);
    const name = draft.name.trim() || preset?.name || "";
    return submit({ type: "save", args: {
      provider: {
        id: provider?.id ?? 0,
        kind: provider ? undefined : draft.kind,
        name,
        base_url: draft.base_url.trim(),
        api_key: key.trim() || null,
        clear_key: clearKey || undefined,
        protocol: draft.protocol,
        // 省略即保留原值（新建用预设），只有改过才提交。
        compat_json: sameJson(initial.compat_json, draft.compat_json) ? undefined : draft.compat_json,
      },
    } }, "保存失败。", ({ savedId }) => { onSaved(savedId, `已保存连接「${name}」。`, !provider); onClose(); });
  };

  const remove = () => {
    if (!provider || busy) return;
    const models = pool?.pool.filter((e) => e.provider_id === provider.id).length ?? 0;
    const uses = bindings?.tasks.filter((t) => t.provider_id === provider.id).map((t) => t.title) ?? [];
    const impact = [
      models ? `它的 ${models} 个模型一起移除` : "",
      uses.length ? `这些用途变为未选择：${uses.join("、")}` : "",
      "单独选过它的模型的对话，之后改用对话默认模型",
    ].filter(Boolean).join("；");
    onAsk(`删除「${provider.name}」`, `删除后：${impact}。`, () => {
      void submit({ type: "delete", id: provider.id }, "删除失败，连接仍保留。", ({ reread }) => {
        onSaved(provider.id, reread ? `已删除「${provider.name}」，但列表刷新失败，请重新读取。` : `已删除「${provider.name}」。`, false);
        onClose();
      });
    }, { okLabel: "删除" });
  };

  const waiting = probe.wait;
  return (
    <Dialog title={label} dirty={dirty} busy={busy} onAsk={onAsk} onClose={onClose} onSave={save} onEditStateChange={onEditStateChange}
      leading={<>
        {provider && <button type="button" className="btn danger" disabled={busy} onClick={remove}>删除连接</button>}
        {waiting && <button type="button" className="btn ghost" onClick={probe.stop}>停止等待</button>}
      </>}
      actions={<>
        <button type="button" className="btn" disabled={busy || !!waiting} aria-busy={!!waiting || undefined} onClick={() => void test()}>
          测试连接
        </button>
        <button type="button" className="btn primary" disabled={busy} aria-busy={busy || undefined} onClick={() => void save()}>
          {provider ? "保存" : "保存并选模型"}
        </button>
      </>}>
      {!provider && (
        <Field label="服务商">
          <ChipBar value={draft.kind} options={presets.map((p) => ({ id: p.id, label: p.name }))} onChange={choosePreset} disabled={busy} />
        </Field>
      )}
      <Field label="名称">
        <input value={draft.name} disabled={busy} placeholder="例如：本机 Ollama" onChange={(e) => change("name", e.target.value)} />
      </Field>
      {replacingKey ? (
        <>
          {provider?.key === "missing" && <p className="notice warn">从不含密钥的备份恢复后需要重新填写 API Key。</p>}
          <SecretField label="API Key" value={key} onChange={(v) => { setKey(v); setClearKey(false); setResult(null); }} disabled={busy}
            placeholder={draft.kind === "ollama" ? "本机服务通常不需要" : "sk-…"} />
          {provider?.key === "set" && (
            <button type="button" className="link-btn" disabled={busy} onClick={() => { setKey(""); setReplacingKey(false); }}>保留原来的 API Key</button>
          )}
        </>
      ) : (
        <Field label="API Key" hint="为了安全，保存后不再显示完整密钥。">
          <div className="row key-saved">
            <span className="mono">{clearKey ? "保存后清除" : provider?.key_hint ? `已保存 · ${provider.key_hint}` : "已保存"}</span>
            <button type="button" className="btn sm" disabled={busy} onClick={() => { setReplacingKey(true); setClearKey(false); }}>更换</button>
            {!clearKey && <button type="button" className="btn sm ghost" disabled={busy} onClick={() => setClearKey(true)}>清除</button>}
          </div>
        </Field>
      )}
      <details className="settings-dialog-advanced" open={!preset?.base_url || undefined}>
        <summary>高级：地址、协议与兼容</summary>
        <Field label="地址" hint="服务的 API 根地址。预设已经填好；Ollama 装在别的电脑上时改这里。">
          <input value={draft.base_url} disabled={busy} spellCheck={false} placeholder="https://api.example.com/v1"
            onChange={(e) => change("base_url", e.target.value)} />
        </Field>
        <Field label="协议" hint="chat 适用于绝大多数兼容服务。">
          <ChipBar className="segmented" value={draft.protocol} options={PROTOCOLS} disabled={busy} onChange={(v) => change("protocol", v)} />
        </Field>
        <Field label="兼容开关" hint="接口兼容选项，一般不用填。在「管理模型」里测试能力后会给出推荐值。">
          <CompatEditor value={draft.compat_json} onChange={(v) => change("compat_json", v)} disabled={busy} label={`${draft.name || "连接"}的兼容设置`} />
        </Field>
        {!atDefaults && defaults && (
          <button type="button" className="link-btn" disabled={busy} onClick={() => setDraft((old) => ({ ...old, ...defaults }))}>恢复预设值</button>
        )}
      </details>
      <div aria-live="polite">
        {waiting ? <p className="hint" role="status">{probeWaitLabel(waiting)}</p> : result ? <Feedback result={result} /> : null}
      </div>
    </Dialog>
  );
}

export function Feedback({ result }: { result: Result }) {
  return (
    <div className={result.tone === "err" ? "form-status error" : "form-status"} role={result.tone === "err" ? "alert" : "status"}>
      {result.text}
      {result.detail ? <details className="ai-error-details"><summary>技术详情</summary><pre>{result.detail}</pre></details> : null}
    </div>
  );
}
