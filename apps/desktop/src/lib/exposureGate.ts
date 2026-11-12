const minRatio = 0.5;
const minMs = 1000;

type ExposureOptions = {
  onExpose: (articleId: number, position: number, shownAt: number) => void;
};

/** 连续可见时间的纯逻辑；forget 只丢弃观察状态，已曝光登记保留。时间单位为毫秒。 */
export function createExposureGate({ onExpose }: ExposureOptions) {
  const rows = new Map<number, { position: number; ratio: number; since: number | null }>();
  const exposed = new Set<number>();
  let active = true;

  return {
    update(articleId: number, position: number, ratio: number, now: number) {
      if (exposed.has(articleId)) return;
      const previous = rows.get(articleId);
      rows.set(articleId, {
        position, ratio,
        since: active && ratio >= minRatio ? previous?.since ?? now : null,
      });
    },
    setActive(next: boolean, now: number) {
      if (active === next) return;
      active = next;
      for (const row of rows.values()) row.since = active && row.ratio >= minRatio ? now : null;
    },
    tick(now: number) {
      for (const [articleId, row] of rows) {
        if (row.since === null || now - row.since < minMs) continue;
        exposed.add(articleId);
        rows.delete(articleId);
        onExpose(articleId, row.position, row.since);
      }
    },
    forget(articleId: number) {
      rows.delete(articleId);
    },
    /** 供调用方安排唯一的下一次唤醒；没有计时中的文章时返回 null。 */
    nextDeadline(): number | null {
      let next: number | null = null;
      for (const row of rows.values()) {
        if (row.since !== null) next = Math.min(next ?? Infinity, row.since + minMs);
      }
      return next;
    },
  };
}
