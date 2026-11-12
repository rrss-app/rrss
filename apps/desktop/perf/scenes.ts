/** F0.2 三档合成场景：规模、测量窗口与起始目标的唯一数值来源。 */

export const SCENE_NAMES = ["minimal", "daily", "stress"] as const;
export type SceneName = (typeof SCENE_NAMES)[number];

export const PERF_SEED = 20092026;
export const PERF_PORT = 1431;
export const PERF_ORIGIN = `http://127.0.0.1:${PERF_PORT}`;
export const PERF_MEDIA_PATH = "/__rrss_perf_media";

/** 固定日历起点，生成标签不用 Date.now()。 */
export const PERF_TIME_ORIGIN_MS = Date.UTC(2024, 0, 1, 8, 0, 0);

export const P95_ALGORITHM = "nearest-rank" as const;
export const P95_ALGORITHM_NOTE =
  "升序后取 index = ceil(p * n) - 1，夹在 [0, n-1]；n=0 时为 null。离散最近秩，不做线性插值。";

export type PerfTargets = {
  /** 热态交互反馈起始目标（dispatch → DOM 可见 / 下一帧的代理，不是 INP）。 */
  interactionP95Ms: number;
  /** 高频路径 JS 工作起始目标。本基线 jsP95 标未测，不把 ScriptDuration 累积或跨 scenario p95 与该值比较。 */
  jsWorkP95Ms: number;
  /** 相对既有基线恶化超过该比例触发复核（本轮只报告，不在默认 suite 失败）。 */
  reviewThreshold: number;
};

export const PERF_TARGETS: PerfTargets = {
  interactionP95Ms: 100,
  jsWorkP95Ms: 8,
  reviewThreshold: 0.2,
};

export const MIN_SAMPLES = 20;
export const MIN_STREAM_SNAPSHOTS = 60;
export const STREAM_INTERVAL_MS = 30;
export const IMAGE_DECODE_TIMEOUT_MS = 10_000;

export type SceneSpec = {
  name: SceneName;
  seed: number;
  articleCount: number;
  /** 模拟宿主分页大小；生产 FOCUS_PAGE_SIZE=40。最小档用 20 以便 40 篇也能续页。 */
  pageSize: number;
  historyMessageCount: number;
  bodyBlockCount: number;
  imageCount: number;
  streamSnapshotCount: number;
  streamIntervalMs: number;
  warmup: number;
  samples: number;
  viewSwitchRepeats: number;
  targets: PerfTargets;
};

/** 三档规模。真实宿主灌库复用这些篇数 / 消息数 / 块数 / 图数，见 scripts/dev-smoke.py。 */
export const SCENES: Record<SceneName, SceneSpec> = {
  minimal: {
    name: "minimal",
    seed: PERF_SEED,
    articleCount: 80,
    pageSize: 20,
    historyMessageCount: 20,
    bodyBlockCount: 40,
    imageCount: 4,
    streamSnapshotCount: MIN_STREAM_SNAPSHOTS,
    streamIntervalMs: STREAM_INTERVAL_MS,
    warmup: 2,
    samples: MIN_SAMPLES,
    viewSwitchRepeats: MIN_SAMPLES,
    targets: PERF_TARGETS,
  },
  daily: {
    name: "daily",
    seed: PERF_SEED,
    articleCount: 500,
    pageSize: 40,
    historyMessageCount: 200,
    bodyBlockCount: 400,
    imageCount: 24,
    streamSnapshotCount: MIN_STREAM_SNAPSHOTS,
    streamIntervalMs: STREAM_INTERVAL_MS,
    warmup: 2,
    samples: MIN_SAMPLES,
    viewSwitchRepeats: MIN_SAMPLES,
    targets: PERF_TARGETS,
  },
  stress: {
    name: "stress",
    seed: PERF_SEED,
    articleCount: 3000,
    pageSize: 40,
    historyMessageCount: 800,
    bodyBlockCount: 1200,
    imageCount: 64,
    streamSnapshotCount: MIN_STREAM_SNAPSHOTS,
    streamIntervalMs: STREAM_INTERVAL_MS,
    warmup: 2,
    samples: MIN_SAMPLES,
    viewSwitchRepeats: MIN_SAMPLES,
    targets: PERF_TARGETS,
  },
};

export const LONG_ARTICLE_ID = 1;
export const LONG_ARTICLE_TITLE = "合成文章 0001 · 长文多图";
export const LONG_THREAD_ID = 1001;
export const LONG_THREAD_TITLE = "合成长会话";
export const DIGEST_THREAD_ID = 301;

export function sceneSpec(name: SceneName): SceneSpec {
  return SCENES[name];
}

export function isSceneName(value: string): value is SceneName {
  return (SCENE_NAMES as readonly string[]).includes(value);
}

export function selectedScenes(raw: string | undefined): SceneName[] {
  const trimmed = raw?.trim();
  if (!trimmed || trimmed === "all") return [...SCENE_NAMES];
  if (!isSceneName(trimmed)) {
    throw new Error(`unknown PERF_SCENE: ${trimmed}`);
  }
  return [trimmed];
}
