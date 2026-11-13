/**
 * 设置选项注册表的前端一半（阶段 19-1）：以 `id` 为键维护文案与展示方式。
 * 类型、范围、默认值与生效方式来自后端；生成的 SettingId 在类型检查时核对注册项。
 */
import type { SettingsSchema } from "../bindings";
import { i18n } from "../i18n";
import { SETTING_SPECS, type SettingId } from "./settingsSchemaData";
export type { SettingId } from "./settingsSchemaData";

/** 预览与测试的独立快照；只覆盖场景当前值，不复制描述符。 */
export function createSettingsSchema(values: Partial<Record<SettingId, string>> = {}): SettingsSchema {
  return {
    specs: structuredClone(SETTING_SPECS),
    values: SETTING_SPECS.map(s => ({ id: s.id, value: values[s.id as SettingId] ?? s.default })),
  };
}

export type RegistryEntry = {
  /** 行标签。 */
  label: string;
  /** 行说明；枚举项自带的说明会接在后面。 */
  hint?: string;
  /** 控件的无障碍名称；缺省用 `label`。 */
  control?: string;
  /**
   * 展示方式：`time` 把「零点起的分钟数」显示成时刻输入框；`minutes` 把秒数按分钟编辑；
   * `multiline` 把文本显示成多行输入框；`wan` 把 token 数按「万 token / 天」编辑（可填小数）。
   * 只影响展示，保存方式仍看描述符的 `apply`。
   */
  display?: "time" | "minutes" | "multiline" | "wan";
  /** 开关两态的文字。 */
  on?: string;
  off?: string;
  /** 枚举各项的文字；后端只登记取值时由这里给（新设置项都这样，展示文案不进后端）。 */
  options?: Record<string, string>;
};

/** 注册表里记的是「有哪些文案、怎么展示」；文字在语言包的 `registry.<设置 id>.*`，读取时才取。 */
type RegistryShape = {
  hint?: true;
  control?: true;
  display?: RegistryEntry["display"];
  on?: true;
  off?: true;
  /** 枚举各项的取值，文字在 `registry.<id>.options.<取值>`。 */
  options?: readonly string[];
};

export const REGISTRY = {
  web_javascript: { hint: true },
  web_open_mode: { hint: true },
  web_original_tip: {},
  web_first_tip: {},
  allow_embedding_model_change: { hint: true },
  feed_icons_enabled: { hint: true },
  feed_fetch_per_host: { hint: true, control: true },
  theme_preference: {},
  focus_filter: {},
  focus_columns: {},
  list_scope: {},
  feeds_scope: {},
  feeds_list_scope: {},
  feeds_pinned: {},
  cluster_group_enabled: {},
  interest_boost_enabled: {},
  rank_features_enabled: {},
  rank_source_enabled: {},
  rank_coverage_enabled: {},
  rank_freshness_enabled: {},
  rank_affinity_enabled: {},
  focus_show_details: {},
  hidden_spot_check_rate: {},
  focus_rerank_enabled: {},
  focus_hide_noisy: {},
  hide_threshold: {},
  translate_target_lang: {},
  developer_tools: {},
  fetch_url_enabled: {},
  ui_debug_mode: { hint: true, on: true, off: true },
  focus_auto_summary: {},
  cluster_auto_summary: {},
  ai_daily_budget_tokens: { hint: true, control: true, display: "wan" },
  ai_embed_daily_budget_tokens: { hint: true, control: true, display: "wan" },
  focus_summary_neutral_limit: { hint: true, control: true },
  ai_input_tier: { hint: true },
  digest_generate_minutes: { hint: true, control: true, display: "time" },
  pi_idle_timeout_secs: { hint: true, control: true, display: "minutes" },
  pi_assistant_note: { hint: true, display: "multiline" },
  pi_auto_download: { hint: true, control: true, on: true, off: true },
  pi_auto_update: { hint: true, control: true, on: true, off: true },
  close_action: { hint: true, options: ["ask", "tray", "quit"] },
  pi_cache_warming: { hint: true },
} satisfies Record<SettingId, RegistryShape>;

/** 语言包里按设置 id 拼出的可选文案键，类型系统只核对必有的 `label`。 */
const text = (id: string, field: string): string => i18n.t(`registry.${id}.${field}` as never, { ns: "settingsSystem" });

/** 在调用时按当前语言取文案，不在模块加载时求值。 */
export function registryEntry(id: string): RegistryEntry | undefined {
  const shape = (REGISTRY as Record<string, RegistryShape>)[id];
  if (!shape) return undefined;
  return {
    label: i18n.t(`registry.${id as SettingId}.label`, { ns: "settingsSystem" }),
    hint: shape.hint ? text(id, "hint") : undefined,
    control: shape.control ? text(id, "control") : undefined,
    display: shape.display,
    on: shape.on ? text(id, "on") : undefined,
    off: shape.off ? text(id, "off") : undefined,
    options: shape.options && Object.fromEntries(shape.options.map((value) => [value, text(id, `options.${value}`)])),
  };
}

/** 零点起的分钟数 ↔ `HH:MM`。 */
export function formatMinutes(minutes: number) {
  const clamped = Math.min(23 * 60 + 59, Math.max(0, Math.trunc(minutes)));
  return `${String(Math.floor(clamped / 60)).padStart(2, "0")}:${String(clamped % 60).padStart(2, "0")}`;
}

export function parseMinutes(value: string) {
  const match = /^(\d{2}):(\d{2})(?::\d{2})?$/.exec(value);
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (hour > 23 || minute > 59) return null;
  return hour * 60 + minute;
}
