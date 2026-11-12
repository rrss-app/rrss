/**
 * 设置选项注册表的前端一半（阶段 19-1）：以 `id` 为键维护文案与展示方式。
 * 类型、范围、默认值与生效方式来自后端；生成的 SettingId 在类型检查时核对注册项。
 */
import type { SettingsSchema } from "../bindings";
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
};

export const REGISTRY = {
  web_javascript: { label: "允许原网页运行 JavaScript", hint: "更改后对新打开的原网页生效。" },
  web_open_mode: { label: "点击外部链接", hint: "右键链接随时可以选别的方式。" },
  web_original_tip: { label: "打开原网页前提示" },
  web_first_tip: { label: "打开时提示不要轻信网页" },
  allow_embedding_model_change: { label: "允许更换嵌入模型", hint: "更换需要全量重建向量库。" },
  feed_icons_enabled: { label: "显示网站图标", hint: "图标保存在本地，关闭后不再下载。" },
  feed_fetch_per_host: {
    label: "每个网站同时下载",
    hint: "同一网站的订阅最多同时下载几个。调高能更快刷新同站的大量订阅（如自建 RSSHub），太高可能被对方限流，限流后该网站会暂停一段时间。",
    control: "每个网站同时下载几个订阅（1–8）",
  },
  theme_preference: { label: "外观" },
  focus_filter: { label: "焦点筛选" },
  focus_columns: { label: "焦点栏目" },
  list_scope: { label: "列表范围" },
  feeds_scope: { label: "订阅范围" },
  feeds_list_scope: { label: "订阅列表范围" },
  feeds_pinned: { label: "置顶" },
  cluster_group_enabled: { label: "聚类分组" },
  interest_boost_enabled: { label: "兴趣加权" },
  rank_features_enabled: { label: "特征偏好" },
  rank_source_enabled: { label: "源偏好" },
  rank_coverage_enabled: { label: "多源报道" },
  rank_freshness_enabled: { label: "时效衰减" },
  rank_affinity_enabled: { label: "和常读文章相似" },
  focus_show_details: { label: "推荐详情" },
  hidden_spot_check_rate: { label: "隐藏抽查比例" },
  focus_rerank_enabled: { label: "关注重排" },
  focus_hide_noisy: { label: "隐藏噪声" },
  hide_threshold: { label: "隐藏阈值" },
  translate_target_lang: { label: "翻译目标" },
  developer_tools: { label: "开发者工具" },
  fetch_url_enabled: { label: "网页读取" },
  ui_debug_mode: {
    label: "界面调试模式",
    hint: "显示阅读器翻译进度，固定在工具栏下方。默认关闭。",
    on: "已开启",
    off: "已关闭",
  },
  focus_auto_summary: { label: "自动摘要" },
  cluster_auto_summary: { label: "自动汇总相近报道" },
  ai_daily_budget_tokens: {
    label: "生成日预算",
    hint: "自动摘要、自动简报等后台生成任务每天最多用这么多 token，用完当天暂停，次日恢复。手动「本篇摘要」和手动简报计入用量，但不受限。0 表示不限额。",
    control: "生成日预算（万 token，0 = 不限额）",
    display: "wan",
  },
  ai_embed_daily_budget_tokens: {
    label: "嵌入日预算",
    hint: "只计嵌入请求（相似文章、兴趣和记忆检索用的向量）。用完只停嵌入，不影响摘要和简报；「立即嵌入」也受它限制。0 表示不限额。",
    control: "嵌入日预算（万 token，0 = 不限额）",
    display: "wan",
  },
  focus_summary_neutral_limit: {
    label: "中性未读额外摘要",
    hint: "除高分文章外，再为这么多条中性未读文章生成摘要；同样只看最近入库的文章，计入自动摘要的每日篇数。0 表示不额外生成。",
    control: "中性未读额外摘要条数（0 = 关）",
  },
  ai_input_tier: {
    label: "每次送给模型的正文",
    hint: "切换只影响之后生成的摘要和简报，已有摘要不会自动重跑。",
  },
  digest_generate_minutes: {
    label: "自动生成",
    hint: "每天这个时间之后生成本日日报；周报在周一的这个时间之后生成。应用没开着会在下次打开时补上。简报里的「生成本期」仍可随时生成。",
    control: "简报自动生成时间",
    display: "time",
  },
  pi_idle_timeout_secs: {
    label: "空闲多久释放（分钟）",
    hint: "没有对话这么久后关闭对话引擎、释放内存，下次发送时自动启动；0 表示不释放。",
    control: "空闲多久释放（分钟）",
    display: "minutes",
  },
  pi_assistant_note: {
    label: "助手说明",
    hint: "每次对话都会带上，适合写回答风格、常用语言这类偏好。",
    display: "multiline",
  },
  pi_auto_download: {
    label: "自动下载",
    hint: "缺少对话引擎时，允许应用自动下载。",
    control: "自动下载对话引擎",
    on: "已开启",
    off: "已关闭",
  },
  pi_auto_update: {
    label: "自动更新",
    hint: "在应用支持的版本范围内更新对话引擎。",
    control: "自动更新对话引擎",
    on: "已开启",
    off: "已关闭",
  },
  pi_cache_warming: {
    label: "缓存保温",
    hint: "在对话之外保持服务商的提示缓存，可以让下次回复更快、更省，但会额外计费。",
  },
} satisfies Record<SettingId, RegistryEntry>;

export function registryEntry(id: string): RegistryEntry | undefined {
  return (REGISTRY as Record<string, RegistryEntry>)[id];
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
