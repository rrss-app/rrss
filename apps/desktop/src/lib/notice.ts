import { formatError } from "./errors";

/** 给 Toast 用的短句。不要把模型整段胡话或 JSON 原文摊进界面。 */
export function formatNotice(prefix: string, err: unknown): string {
  const compact = formatError(err).replace(/\s+/g, " ");
  if (compact.length > 140) {
    return `${prefix}${compact.slice(0, 137)}…`;
  }
  return `${prefix}${compact}`;
}
