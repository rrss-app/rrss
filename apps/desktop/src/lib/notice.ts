/** 给 Toast 用的短句。不要把模型整段胡话或 JSON 原文摊进界面。 */
export function formatNotice(prefix: string, err: unknown): string {
  const raw = String(err).replace(/^Error:\s*/, "").trim();
  if (/invalid JSON/i.test(raw)) {
    return `${prefix}模型没有返回可用的 JSON，请换模型或重试`;
  }
  if (/database is locked|database lock|sqlite_busy/i.test(raw)) {
    return `${prefix}数据库正忙，请稍后重试`;
  }
  const compact = raw.replace(/\s+/g, " ");
  if (compact.length > 140) {
    return `${prefix}${compact.slice(0, 137)}…`;
  }
  return `${prefix}${compact}`;
}
