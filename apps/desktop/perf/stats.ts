import { P95_ALGORITHM, P95_ALGORITHM_NOTE, type PerfTargets } from "./scenes";

export type SampleSummary = {
  n: number;
  raw: number[];
  min: number | null;
  max: number | null;
  mean: number | null;
  p50: number | null;
  p95: number | null;
  algorithm: typeof P95_ALGORITHM;
  algorithmNote: string;
};

export type TargetCheck = {
  metric: string;
  p95: number | null;
  targetMs: number;
  over: boolean;
  unmeasured: boolean;
};

/** 最近秩分位。p 落在 (0, 1]。 */
export function percentileNearestRank(samples: readonly number[], p: number): number | null {
  if (samples.length === 0) return null;
  if (!(p > 0) || p > 1) throw new Error("percentile p must be in (0, 1]");
  const sorted = [...samples].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1));
  return sorted[index] ?? null;
}

export function summarizeSamples(samples: readonly number[]): SampleSummary {
  const raw = [...samples];
  const n = raw.length;
  const sum = raw.reduce((acc, value) => acc + value, 0);
  return {
    n,
    raw,
    min: n === 0 ? null : Math.min(...raw),
    max: n === 0 ? null : Math.max(...raw),
    mean: n === 0 ? null : sum / n,
    p50: percentileNearestRank(raw, 0.5),
    p95: percentileNearestRank(raw, 0.95),
    algorithm: P95_ALGORITHM,
    algorithmNote: P95_ALGORITHM_NOTE,
  };
}

export function checkTarget(
  metric: string,
  p95: number | null,
  targetMs: number,
  unmeasured = false,
): TargetCheck {
  return {
    metric,
    p95,
    targetMs,
    over: !unmeasured && p95 !== null && p95 > targetMs,
    unmeasured,
  };
}

export function collectOverruns(checks: readonly TargetCheck[]): TargetCheck[] {
  return checks.filter((check) => check.over);
}

export function reviewDelta(current: number, baseline: number): number {
  if (baseline === 0) return current === 0 ? 0 : Infinity;
  return (current - baseline) / baseline;
}

export function exceedsReviewThreshold(current: number, baseline: number, targets: PerfTargets): boolean {
  return reviewDelta(current, baseline) > targets.reviewThreshold;
}
