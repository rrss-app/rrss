import type { FeedHealthNotice, FeedView, UiError } from "../../bindings";
import { i18n, locale } from "../../i18n";
import { asUiError, formatError } from "../../lib/errors";

const count = (value: number) => new Intl.NumberFormat(locale(), { useGrouping: false }).format(value);

function httpError(status: number): string {
  const key = status === 429 ? "limited" : status === 401 || status === 403 ? "denied"
    : status === 404 ? "missing" : [502, 503, 504].includes(status) ? "unavailable" : "other";
  return i18n.t(`health.http.${key}`, { ns: "feeds", status: count(status) });
}

/** Keep the old display projection while the original text remains in legacy data. */
function legacyFeedError(raw: string): string {
  if (/database is locked|database lock|sqlite_busy|code: 5/i.test(raw)) return i18n.t("database_busy", { ns: "errors" });
  const digits = /^HTTP (\d+)/.exec(raw)?.[1] ?? /^\d+/.exec(raw.split("status=")[1] ?? "")?.[0];
  if (digits && Number(digits) <= 65535) return httpError(Number(digits));
  const compact = raw.split(/\s+/).filter(Boolean).join(" ");
  const chars = Array.from(compact);
  if (chars.length > 80) return i18n.t("health.truncated", { ns: "feeds", text: chars.slice(0, 77).join("") });
  return compact || i18n.t("feed_refresh_failed", { ns: "errors" });
}

/** Match the existing host's local date projection; timestamps stay raw in IPC. */
function healthDate(ms: number, now = Date.now()): string {
  const date = new Date(ms);
  if (!Number.isFinite(date.getTime())) return "";
  const today = new Date(now);
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  const sameDay = (other: Date) => date.getFullYear() === other.getFullYear()
    && date.getMonth() === other.getMonth() && date.getDate() === other.getDate();
  const time = new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(date);
  if (sameDay(today)) return i18n.t("health.date.today", { ns: "feeds", time });
  if (sameDay(yesterday)) return i18n.t("health.date.yesterday", { ns: "feeds", time });
  const params = { year: count(date.getFullYear()), month: count(date.getMonth() + 1), day: count(date.getDate()), time };
  return date.getFullYear() === today.getFullYear()
    ? i18n.t("health.date.monthDay", { ns: "feeds", ...params })
    : i18n.t("health.date.yearDay", { ns: "feeds", ...params });
}

export function formatFeedError(error: UiError | null): string {
  if (!error) return "";
  const value = asUiError(error);
  if (value.code === "legacy_text") return legacyFeedError(value.detail);
  if (value.code === "feed_timeout") return i18n.t("health.timeout", { ns: "feeds" });
  const policyHttp = ["feed_body_too_large", "feed_redirect_limit", "feed_redirect_missing",
    "feed_redirect_invalid", "feed_redirect_scheme", "feed_address_invalid", "feed_runtime_stopped"];
  if (value.code === "feed_refresh_http" || value.code.startsWith("network_") || policyHttp.includes(value.code)) {
    return httpError(Number(value.params.status ?? 0));
  }
  return formatError(error);
}

export function feedHealthKind(health: FeedHealthNotice | null): "failing" | "stale" | "noisy" | "" {
  switch (health?.code) {
    case "feed_health_failing": return "failing";
    case "feed_health_stale": return "stale";
    case "feed_health_noisy": return "noisy";
    default: return "";
  }
}

export function feedHealthLabel(health: FeedHealthNotice | null): string {
  switch (health?.code) {
    case "feed_health_failing": return isInitialFailure(health)
      ? i18n.t("health.feed_health_failing.initialLabel", { ns: "feeds" })
      : i18n.t("health.feed_health_failing.label", { ns: "feeds",
      failures: count(health.params.fail_count), days: count(health.params.failing_days) });
    case "feed_health_stale": return i18n.t("health.feed_health_stale.label", { ns: "feeds", days: count(health.params.quiet_days) });
    case "feed_health_noisy": return i18n.t("health.feed_health_noisy.label", { ns: "feeds" });
    default: return "";
  }
}

function isInitialFailure(health: FeedHealthNotice): boolean {
  return health.code === "feed_health_failing" && health.params.fail_count === 1
    && health.params.failing_days === 0 && health.params.last_success_at == null;
}

export function feedHealthDetail(feed: FeedView, now = Date.now()): string {
  const health = feed.health;
  if (!health) return "";
  if (isInitialFailure(health)) return formatFeedError(feed.error);
  if (health.code === "feed_health_noisy") return i18n.t("health.feed_health_noisy.detail", { ns: "feeds", days: count(health.params.window_days) });
  const params = health.params;
  const success = params.last_success_at == null ? i18n.t("health.neverSucceeded", { ns: "feeds" })
    : i18n.t("health.recentSuccess", { ns: "feeds", date: healthDate(params.last_success_at, now) });
  if (health.code === "feed_health_failing") return i18n.t("health.feed_health_failing.detail", { ns: "feeds", error: formatFeedError(feed.error), success });
  const pace = health.params.avg_interval_days == null ? "" : i18n.t("health.feed_health_stale.pace", { ns: "feeds", days: count(Math.max(1, Math.round(health.params.avg_interval_days))) });
  const last = health.params.last_article_at == null ? i18n.t("health.feed_health_stale.noArticles", { ns: "feeds" })
    : i18n.t("health.feed_health_stale.lastArticle", { ns: "feeds", date: healthDate(health.params.last_article_at, now) });
  const fetch = i18n.t(health.params.failed ? "health.feed_health_stale.fetchFailed" : "health.feed_health_stale.fetchOk", { ns: "feeds" });
  return i18n.t("health.feed_health_stale.detail", { ns: "feeds", pace, last, fetch, success });
}
