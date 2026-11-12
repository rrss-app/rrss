import { useSyncExternalStore } from "react";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

const pad = (n: number) => String(n).padStart(2, "0");

function startOfDay(ms: number, offsetDays = 0): number {
  const day = new Date(ms);
  return new Date(day.getFullYear(), day.getMonth(), day.getDate() + offsetDays).getTime();
}

/**
 * 发布时间的列表说法，按本机时区：今天用「刚刚 / N 分钟前 / N 小时前」，昨天用「昨天 HH:mm」，
 * 更早用本地日期，跨年带年份。未来时间（时钟偏差）超过一分钟时按日期显示。
 */
export function formatPublished(ms: number | null | undefined, now: number = Date.now()): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  const diff = now - ms;
  if (Math.abs(diff) < MINUTE) return "刚刚";
  const today = startOfDay(now);
  const date = new Date(ms);
  if (diff > 0 && ms >= today) return diff < HOUR ? `${Math.floor(diff / MINUTE)} 分钟前` : `${Math.floor(diff / HOUR)} 小时前`;
  if (ms >= startOfDay(now, -1) && ms < today) return `昨天 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const day = `${date.getMonth() + 1}月${date.getDate()}日`;
  return date.getFullYear() === new Date(now).getFullYear() ? day : `${date.getFullYear()}年${day}`;
}

/** 按本地日期分组的组头：今天 / 昨天 / 具体日期（跨年带年份）。 */
export function formatDay(ms: number | null | undefined, now: number = Date.now()): string {
  if (ms == null || !Number.isFinite(ms)) return "未注明时间";
  const today = startOfDay(now);
  if (ms >= today) return "今天";
  if (ms >= startOfDay(now, -1)) return "昨天";
  const date = new Date(ms);
  const day = `${date.getMonth() + 1}月${date.getDate()}日`;
  return date.getFullYear() === new Date(now).getFullYear() ? day : `${date.getFullYear()}年${day}`;
}

/** 完整的本地时间，给悬停提示和不需要相对说法的地方。 */
export function formatPublishedFull(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  const date = new Date(ms);
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

// 共用一个分钟时钟：有订阅者时才计时，保活的列表隔一会儿再看也不会停在旧的相对时间。
const listeners = new Set<() => void>();
let now = Date.now();
let timer: ReturnType<typeof setInterval> | undefined;

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (timer === undefined) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      for (const notify of [...listeners]) notify();
    }, MINUTE);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

// 没有订阅者时时钟不走，首次渲染取当前时刻，避免先画出过时的说法；
// 一秒内重复读取返回同一个值，满足 useSyncExternalStore 对快照稳定的要求。
function getNow() {
  if (timer === undefined && Math.abs(Date.now() - now) > 1000) now = Date.now();
  return now;
}

export function useMinuteClock(): number {
  return useSyncExternalStore(subscribe, getNow, getNow);
}
