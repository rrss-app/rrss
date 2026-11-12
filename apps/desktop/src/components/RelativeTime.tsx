import { formatPublished, formatPublishedFull, useMinuteClock } from "../lib/time";

/** 按本机时区显示的发布时间；只有这一小段随分钟时钟重画。 */
export function RelativeTime({ ms, className }: { ms: number; className?: string }) {
  const now = useMinuteClock();
  return <time className={className} dateTime={new Date(ms).toISOString()} title={formatPublishedFull(ms)}>{formatPublished(ms, now)}</time>;
}
