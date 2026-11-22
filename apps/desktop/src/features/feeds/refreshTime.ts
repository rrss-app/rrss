import { locale } from "../../i18n";

const DAY = 86_400_000;

function startOfDay(ms: number): number {
  const day = new Date(ms);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

/**
 * A refresh time by distance from now: today shows only the time, yesterday and tomorrow
 * a relative day, the surrounding week a weekday, later dates the date (with the year
 * when it differs). Every tier keeps the time of day.
 */
export function formatRefreshTime(at: number, now = Date.now(), lang = locale()): string {
  const days = Math.round((startOfDay(at) - startOfDay(now)) / DAY);
  const clock = { hour: "2-digit", minute: "2-digit", hourCycle: "h23" } as const;
  const time = new Intl.DateTimeFormat(lang, clock).format(at);
  if (days === 0) return time;
  if (Math.abs(days) === 1) return `${new Intl.RelativeTimeFormat(lang, { numeric: "auto" }).format(days, "day")} ${time}`;
  if (Math.abs(days) < 7) return `${new Intl.DateTimeFormat(lang, { weekday: "short" }).format(at)} ${time}`;
  const year = new Date(at).getFullYear() !== new Date(now).getFullYear() ? { year: "numeric" } as const : {};
  return new Intl.DateTimeFormat(lang, { ...year, month: "short", day: "numeric", ...clock }).format(at);
}
