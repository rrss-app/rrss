import type { AutoSummaryView } from "../bindings";
import { locale } from "../i18n";

/**
 * 自动摘要与预算的估算（AI 规格 · 自动摘要与预算 · 估算）。单篇与简报两个数由后端按档位算好下发，
 * 这里只做「约够 N 篇 / 天」这一步除法与文案；开启确认里改预算草稿时即时重算。
 */

/** 「20 万」「4,000」：一万以上按万写、保留一位小数。 */
export function wan(tokens: number): string {
  return tokens >= 10_000
    ? `${Number((tokens / 10_000).toFixed(1)).toLocaleString(locale(), { maximumFractionDigits: 1 })} 万`
    : tokens.toLocaleString(locale());
}

export const digits = (n: number) => n.toLocaleString(locale());

/** 开启自动汇总时每天最多用掉的 token（组数上限 × 每组）。 */
export function clusterDaily(auto: AutoSummaryView, clusterOn: boolean): number {
  return clusterOn ? auto.cluster_daily_limit * auto.per_cluster_tokens : 0;
}

/** 预算先扣自动简报（和开着的自动汇总），剩下的够写几篇；10 篇以上向下取整到 5。 */
export function articlesFit(auto: AutoSummaryView, budget: number, clusterOn: boolean): number {
  const k = Math.max(0, budget - auto.digest_daily_tokens - clusterDaily(auto, clusterOn)) / Math.max(1, auto.per_article_tokens);
  return k >= 10 ? Math.floor(k / 5) * 5 : Math.floor(k);
}

/** 「扣除自动简报约 3.5 万后约够 40 篇 / 天」；预算大于 0 时用，句末不带「（估算）」。 */
export function fitText(auto: AutoSummaryView, budget: number, clusterOn: boolean): string {
  const digest = auto.digest_daily_tokens;
  const cluster = clusterDaily(auto, clusterOn);
  const what = cluster ? `自动简报约 ${wan(digest)}、自动汇总约 ${wan(cluster)}` : `自动简报约 ${wan(digest)}`;
  if (budget <= digest + cluster) {
    return `只够${cluster ? "自动简报和自动汇总" : "自动简报"}（约 ${wan(digest + cluster)} / 天），自动摘要基本不会运行`;
  }
  const k = articlesFit(auto, budget, clusterOn);
  return k >= auto.daily_limit
    ? `扣除${what}后超过每天 ${auto.daily_limit} 篇的上限，预算不是瓶颈`
    : `扣除${what}后约够 ${k} 篇 / 天`;
}

/** 生成预算为 0 时，开启后每天最多约用多少：上限篇数 × 单篇 + 简报（+ 汇总）。 */
export function unlimitedText(auto: AutoSummaryView, clusterOn: boolean): string {
  const cluster = clusterDaily(auto, clusterOn);
  const most = auto.daily_limit * auto.per_article_tokens + auto.digest_daily_tokens + cluster;
  const extra = `加自动简报约 ${wan(auto.digest_daily_tokens)}${cluster ? `、自动汇总约 ${wan(cluster)}` : ""}`;
  return `自动摘要每天最多约 ${wan(most)} token（${auto.daily_limit} 篇 × 约 ${digits(auto.per_article_tokens)}，${extra}，估算）`;
}

/** 预算存的是 token，输入按「万 token」：库值 → 输入框文字。 */
export function tokensToWanText(tokens: string): string {
  const n = Number(tokens);
  return tokens.trim() !== "" && Number.isFinite(n) ? String(Number((n / 10_000).toFixed(4))) : tokens;
}

/** 输入框文字 → 库值；不是非负数时原样返回，交给保存前的预检报错。 */
export function wanTextToTokens(text: string): string {
  const trimmed = text.trim();
  const n = Number(trimmed);
  return trimmed !== "" && Number.isFinite(n) && n >= 0 ? String(Math.round(n * 10_000)) : text;
}
