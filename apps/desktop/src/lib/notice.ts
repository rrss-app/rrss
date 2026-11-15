import { formatError } from "./errors";

/**
 * 把「前缀：」与后面的说明接起来。前缀键整句带着本语言的冒号（中文「：」、英文「:」），
 * 英文冒号后补一个空格，译文不用在 JSON 里留行尾空格。
 */
export function withPrefix(prefix: string, text: string): string {
  return /:$/.test(prefix) ? `${prefix} ${text}` : `${prefix}${text}`;
}

/** 给 Toast 用的短句。不要把模型整段胡话或 JSON 原文摊进界面。 */
export function formatNotice(prefix: string, err: unknown): string {
  const compact = formatError(err).replace(/\s+/g, " ");
  return withPrefix(prefix, compact.length > 140 ? `${compact.slice(0, 137)}…` : compact);
}
