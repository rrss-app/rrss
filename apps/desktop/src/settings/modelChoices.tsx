import { useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { ModelPicker, type PickerItem } from "../components/ModelPicker";
import { api, type ModelBindingsView, type ModelChoice, type ModelPoolView, type PoolEntryView, type ProviderView } from "../lib/api";
import { shortModel } from "../lib/fuzzy";
import { formatNotice } from "../lib/notice";
import { aiQueryOptions, bindingsQueryOptions, poolQueryOptions } from "./aiQuery";

// 模型的选择列表（阶段 19-7）：没有主模型与「跟随」，所有用途只从加进来的模型里选，
// 对话只从打开了「用于对话」的里选。键是「连接 id|模型 id」。

export const choiceKey = (c: ModelChoice) => `${c.provider_id}|${c.model}`;
export function parseChoice(key: string): ModelChoice | null {
  const at = key.indexOf("|");
  return at > 0 ? { provider_id: Number(key.slice(0, at)), model: key.slice(at + 1) } : null;
}
export const sameChoice = (a: ModelChoice | null | undefined, b: ModelChoice | null | undefined) =>
  !!a && !!b && a.provider_id === b.provider_id && a.model === b.model;

/** 选择列表与对话输入区显示的名字。 */
export const modelName = (e: Pick<PoolEntryView, "display_name" | "model">) => e.display_name || e.model;
export const findEntry = (pool: ModelPoolView | undefined, c: ModelChoice | null | undefined) =>
  c ? pool?.pool.find((e) => sameChoice(e, c)) : undefined;

const host = (p?: ProviderView) => {
  try { return p ? new URL(p.base_url).host : ""; } catch { return p?.base_url ?? ""; }
};
const connection = (providers: ProviderView[], id: number) => providers.find((p) => p.id === id);

/** 用途当前选的模型；没选时为 null。 */
export function taskChoice(bindings: ModelBindingsView | undefined, task: string): ModelChoice | null {
  const t = bindings?.tasks.find((x) => x.task === task);
  return t && t.provider_id > 0 && t.model ? { provider_id: t.provider_id, model: t.model } : null;
}
export const chatDefault = (bindings: ModelBindingsView | undefined) => taskChoice(bindings, "chat");

/** 按钮上的名字与悬停提示：有显示名原样显示（界面字体，太长由 CSS 截断），没有时用等宽缩写。 */
export function describeChoice(choice: ModelChoice | null, pool: ModelPoolView | undefined, providers: ProviderView[]) {
  if (!choice) return { text: "未选择", title: "还没选模型", plain: true };
  const e = findEntry(pool, choice);
  // 连接列表还没读回时不下结论，只显示模型 id。
  const name = connection(providers, choice.provider_id)?.name ?? (providers.length ? "已删除的连接" : "");
  const title = name ? `${choice.model} · ${name}` : choice.model;
  return e?.display_name ? { text: e.display_name, title, plain: true } : { text: shortModel(choice.model), title };
}

/** 这个用途需要的能力。 */
export type Need = "tools" | "json";
export function lacks(e: PoolEntryView | undefined, need?: Need) {
  if (!e || !need) return "";
  if (need === "tools" && e.effective.tool_calling === "unsupported") return "不能调用工具";
  if (need === "json" && e.effective.structured_output === "unsupported") return "不支持结构化输出";
  return "";
}

/**
 * 候选：加进来的模型（`chat` 时只列用于对话的），按连接分组。`current` 不在其中时仍列出，免得当前项消失。
 * `defaultKey` 给出时在对话默认旁标「默认」（对话中用）。`inThread`（对话中）：主文字与按钮同名（显示名或缩写），
 * 小字总是完整 id，没起显示名的标「自动」，池里的模型可就地改名。
 */
export function modelItems(opts: {
  pool: ModelPoolView | undefined;
  providers: ProviderView[];
  chat?: boolean;
  need?: Need;
  current: ModelChoice | null;
  defaultKey?: string;
  inThread?: boolean;
}): PickerItem[] {
  const { pool, providers, chat, need, current, defaultKey, inThread } = opts;
  const item = (choice: ModelChoice, e: PoolEntryView | undefined, extra?: ReactNode): PickerItem => {
    const p = connection(providers, choice.provider_id);
    const warn = lacks(e, need);
    const named = !!e?.display_name;
    return {
      key: choiceKey(choice),
      ...(inThread
        ? { label: named ? e!.display_name : shortModel(choice.model), plain: named, hint: choice.model,
          rename: e ? { value: e.display_name, placeholder: shortModel(choice.model) } : undefined }
        : { label: e ? modelName(e) : choice.model, hint: named ? e!.model : undefined }),
      tag: p?.name ?? "已删除的连接", tagTitle: host(p), group: p?.name ?? "已删除的连接",
      marks: <>
        {inThread && e && !named && <span className="model-picker-mark faint" title="还没起显示名，用模型 id 的缩写">自动</span>}
        {warn && <span className="model-picker-mark warn" title={need === "tools" ? "对话将无法查阅文章或读取网页" : "结果可能格式错乱"}>{warn}</span>}
        {defaultKey === choiceKey(choice) && <span className="model-picker-mark">默认</span>}
        {extra}
      </>,
    };
  };
  const members = (pool?.pool ?? []).filter((e) => !chat || e.chat).map((e) => item(e, e));
  const extra = current && !members.some((m) => m.key === choiceKey(current))
    ? [item(current, findEntry(pool, current), <span className="model-picker-mark">{findEntry(pool, current) ? "未用于对话" : "已移出"}</span>)]
    : [];
  return [...members, ...extra];
}

/** 思考强度由低到高，与后端 `THINKING_LEVELS` 同序；界面只用英文档位名。 */
export const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
const THINKING_SHORT: Record<string, string> = { minimal: "min", medium: "med" };
export const thinkingLabel = (level: string) => THINKING_SHORT[level] ?? level;
/** 常用四档总列出；引擎握手报告过的更高 / 更细档位也列出。不支持的档位开会话时降到最近一档。 */
export function thinkingOptions(reported: readonly string[] = []) {
  const shown = new Set(["off", "low", "medium", "high", ...reported]);
  return THINKING_LEVELS.filter((l) => shown.has(l)).map((id) => ({ id, label: thinkingLabel(id) }));
}

/** 加进来的模型、连接与用途三份快照；模型页、对话助手页、翻译页与对话输入区共用查询键。 */
export function usePoolChoices(gate?: { enabled: (q: { state: { status: string } }) => boolean }) {
  const pool = useQuery({ ...poolQueryOptions(), ...gate });
  const providers = useQuery({ ...aiQueryOptions(), ...gate }).data?.providers ?? [];
  const bindings = useQuery({ ...bindingsQueryOptions(), ...gate }).data;
  return { pool: pool.data, providers, bindings, poolError: pool.error };
}

/** 一个用途的模型选择。`mixed` 是组里各项模型不同时按钮上的文字。 */
export function ModelMenu({ label, value, onPick, providers, pool, need, chat, heading, onManage, disabled, mixed }: {
  label: string;
  value: ModelChoice | null;
  onPick: (choice: ModelChoice) => void;
  providers: ProviderView[];
  pool: ModelPoolView | undefined;
  need?: Need;
  chat?: boolean;
  heading?: { title: string; hint?: string };
  onManage: () => void;
  disabled?: boolean;
  mixed?: string;
}) {
  const items = modelItems({ pool, providers, chat, need, current: mixed ? null : value });
  const list = (pool?.pool ?? []).filter((e) => !chat || e.chat);
  const display = mixed ? { text: mixed, title: mixed, plain: true } : describeChoice(value, pool, providers);
  return (
    <ModelPicker label={label} value={mixed || !value ? null : choiceKey(value)} items={items} display={display}
      tone={!value && !mixed ? "warn" : undefined} disabled={disabled} heading={heading}
      onPick={(key) => { const c = parseChoice(key); if (c) onPick(c); }}
      notice={list.length ? undefined : <>
        <span>{chat ? "还没有用于对话的模型。在「管理模型」里给模型打开「用于对话」。" : "还没有加进来的模型。"}</span>
        <button type="button" className="btn sm primary" onClick={onManage}>{chat ? "管理模型" : "添加模型"}</button>
      </>}
      footer={list.length ? (close) => <>
        <span className="hint">{chat ? `对话可用 ${list.length} 个` : `已加入 ${list.length} 个`}</span>
        <button type="button" className="btn sm ghost" onClick={() => { close(); onManage(); }}>管理模型</button>
      </> : undefined} />
  );
}

/** 对话默认 / 对话中的选择列表：只列用于对话的模型。 */
export function ChatModelMenu({ value, onPick, providers, pool, bindings, onManage, chat, disabled, onRename }: {
  value: ModelChoice | null;
  onPick: (choice: ModelChoice) => void;
  providers: ProviderView[];
  pool: ModelPoolView | undefined;
  bindings: ModelBindingsView | undefined;
  onManage: () => void;
  /** 对话中：列表向上展开，只影响这条对话，并标出对话默认；`extra` 排在列表下面（思考强度）。 */
  chat?: { badge?: ReactNode; extra?: ReactNode };
  disabled?: boolean;
  /** 对话中就地改显示名：成功返回 null，失败返回原因。 */
  onRename?: (choice: ModelChoice, name: string) => Promise<string | null>;
}) {
  const def = chatDefault(bindings);
  const items = modelItems({ pool, providers, chat: true, need: "tools", current: value, defaultKey: chat && def ? choiceKey(def) : undefined, inThread: !!chat });
  const count = (pool?.pool ?? []).filter((e) => e.chat).length;
  return (
    <ModelPicker label={chat ? "这条对话的模型" : "对话模型"} value={value ? choiceKey(value) : null} items={items}
      onPick={(key) => { const c = parseChoice(key); if (c) onPick(c); }}
      display={describeChoice(value, pool, providers)} badge={chat?.badge} extra={chat?.extra} tone={!value ? "warn" : undefined}
      variant={chat ? "chip" : "field"} placement={chat ? "up" : "down"} disabled={disabled}
      onRename={chat && onRename ? (key, name) => { const c = parseChoice(key); return c ? onRename(c, name) : Promise.resolve(null); } : undefined}
      heading={chat
        ? { title: "这条对话接下来用", hint: "只影响这条对话之后的回复。新对话用对话默认模型。" }
        : { title: "对话默认模型", hint: "新对话用它。对话助手页和模型页的「对话」是同一项。" }}
      notice={count ? undefined : <>
        <span>还没有用于对话的模型。在「管理模型」里给模型打开「用于对话」。</span>
        <button type="button" className="btn sm primary" onClick={onManage}>管理模型</button>
      </>}
      footer={count ? (close) => <>
        <span className="hint">对话可用 {count} 个</span>
        <button type="button" className="btn sm ghost" onClick={() => { close(); onManage(); }}>管理模型</button>
      </> : undefined} />
  );
}

/** 自有 Pi 配置下的对话模型：只列 Pi 的模型。读取列表可能启动 Pi，所以由用户点按钮触发。 */
export function OwnModelMenu({ value, ownModel, onPick, chat, disabled }: {
  /** 当前选择；空表示 Pi 默认。 */
  value: string;
  /** 设置里的自有模型（对话中标「默认」）。 */
  ownModel: string;
  onPick: (model: string) => void;
  chat?: { badge?: ReactNode; extra?: ReactNode };
  disabled?: boolean;
}) {
  const [models, setModels] = useState<string[]>([]);
  const [status, setStatus] = useState("");
  const load = async () => {
    setStatus("读取中…");
    try { setModels(await api.piAvailableModels()); setStatus(""); }
    catch (err) { setStatus(formatNotice("读取 Pi 的模型失败：", err)); }
  };
  const def = (m: string) => (chat && m === ownModel ? <span className="model-picker-mark">默认</span> : null);
  const ids = [...new Set([ownModel, value, ...models].filter(Boolean))];
  const items: PickerItem[] = [
    { key: "", label: "Pi 默认", plain: true, hint: "由你的 Pi 配置决定", marks: def("") },
    ...ids.map((m) => ({ key: m, label: m, marks: def(m) })),
  ];
  return (
    <ModelPicker label={chat ? "这条对话的模型" : "对话模型"} value={value} items={items} onPick={onPick} disabled={disabled}
      display={value ? { text: shortModel(value), title: value } : { text: "Pi 默认", title: "Pi 默认", plain: true }}
      badge={chat?.badge} extra={chat?.extra} variant={chat ? "chip" : "field"} placement={chat ? "up" : "down"}
      heading={{ title: "来自你的 Pi 配置", hint: "已开启「使用我自己的 Pi 配置」，模型列表不用于对话。" }}
      footer={() => <>
        <span className="hint" role="status">{status || (models.length ? `Pi 可用 ${models.length} 个` : "列表需要读取")}</span>
        <button type="button" className="btn sm ghost" onClick={() => void load()}>读取可用模型</button>
      </>} />
  );
}
