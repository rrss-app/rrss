import { useTranslation } from "../i18n";

/** 对话引擎的兼容设置（pi-ai 的 `compat`）：连接弹层与单个模型的「高级」共用。 */

/** 常用的键；勾上即写入「不支持」时的取值，取消即删掉该键。其余键在原始 JSON 里改。 */
const COMPAT_SWITCHES = [
  { key: "supportsDeveloperRole", value: false },
  { key: "supportsReasoningEffort", value: false },
  { key: "supportsStrictMode", value: false },
  { key: "maxTokensField", value: "max_tokens" },
] as const;

export function prettyJson(raw: string | undefined | null) {
  try { return JSON.stringify(JSON.parse(raw || "{}"), null, 2); } catch { return raw ?? ""; }
}

export function sameJson(a: string, b: string) {
  try { return JSON.stringify(JSON.parse(a || "{}")) === JSON.stringify(JSON.parse(b || "{}")); } catch { return a === b; }
}

/** 空白视为空对象；不是 JSON 对象时为 null。 */
export function jsonObject(text: string): Record<string, unknown> | null {
  if (!text.trim()) return {};
  try {
    const value: unknown = JSON.parse(text);
    return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
  } catch { return null; }
}

/** 常用开关 + 折叠的原始 JSON。原始 JSON 不合法时开关禁用，保存前由调用方预检。 */
export function CompatEditor({ value, onChange, disabled, label }: {
  value: string;
  onChange: (text: string) => void;
  disabled?: boolean;
  /** 原始 JSON 输入框的无障碍名称。 */
  label: string;
}) {
  const { t } = useTranslation("settingsModels");
  const compat = jsonObject(value);
  const flip = (key: string, v: unknown, on: boolean) => {
    const next = { ...compat };
    if (on) next[key] = v; else delete next[key];
    onChange(JSON.stringify(next, null, 2));
  };
  return (
    <div className="compat-editor">
      <div className="compat-checks">
        {COMPAT_SWITCHES.map(({ key, value: v }) => (
          <label key={key}>
            <input type="checkbox" checked={compat?.[key] === v} disabled={disabled || compat === null}
              onChange={(e) => flip(key, v, e.target.checked)} />
            {t(`compat.switch.${key}`)} <code>{key}</code>
          </label>
        ))}
      </div>
      <details className="advanced-json" open={compat === null || undefined}>
        <summary>{t("compat.rawJson")}</summary>
        <textarea className="code" rows={5} aria-label={label} value={value} disabled={disabled} spellCheck={false}
          onChange={(e) => onChange(e.target.value)} />
      </details>
      {compat === null && <p className="form-status error" role="alert">{t("compat.invalid")}</p>}
    </div>
  );
}
