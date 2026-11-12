import os from "node:os";
import type { SceneSpec } from "./scenes";
import { PERF_TARGETS } from "./scenes";
import type { SampleSummary, TargetCheck } from "./stats";

export const METRIC_MEANINGS = {
  interactionToNextFrameMs:
    "页面内 performance.now：用户动作 dispatch 到双 rAF 下一帧。代理指标，不是 INP，也不是 Playwright 跨进程延迟。",
  interactionToVisibleMs:
    "页面内 performance.now：dispatch 到约定 DOM 节点可见。同样不是 INP。",
  scriptDurationDeltaMs:
    "Chromium CDP Performance.ScriptDuration 在该次动作测量窗口内的累积增量（秒×1000）。窗口含准备 / 关闭阅读器 / 切回视图等，只当窗口工作量，不是单帧 JS，也不与 8ms 比较。",
  jsP95Ms: "未测。本基线没有 Interaction to Next Paint / 单帧 JS 工作量探针；不把 scriptDuration 或跨 scenario p95 再取 p95 当作 jsP95。",
  ipcLatencyMs: "严格模拟宿主页内 invoke 的耗时。mock，不是真实 Rust IPC。",
  ipcBytes: "完整 JSON 参数 UTF-8 字节（JSON.stringify(args)）。只记录方法名与体量，不存参数内容 / 正文 / key。",
  jsHeapUsedSize: "CDP Performance.JSHeapUsedSize 快照，字节。",
  nodes: "CDP Performance.Nodes 快照。",
  streamEmitToRenderMs:
    "宿主 emit chat:delta 时的 performance.now 到唯一 textLength 相等且时间顺序匹配的 DOM 观测时延。不是 snapshot 间隔 30ms。未匹配计入 missed。",
  coldStartFirstArticleMs:
    "文档 time origin 起至首篇 article 在布局中可见（getClientRects 且非 hidden）由 MutationObserver 记下的时间。不是仅节点插入，也不是 Playwright wait 之后再读 performance.now。",
} as const;

export type MachineInfo = {
  os: string;
  cpu: string;
  cpuCores: number;
  node: string;
  browser: { name: string; version: string };
  build: "production";
  strictModeDevReplay: false;
  unverifiedEngines: ["WebKitGTK", "WebView2"];
};

export function machineInfo(browserVersion: string): MachineInfo {
  const cpus = os.cpus();
  return {
    os: `${os.platform()} ${os.release()} ${os.arch()}`,
    cpu: cpus[0]?.model ?? "unknown",
    cpuCores: cpus.length,
    node: process.version,
    browser: { name: "chromium", version: browserVersion },
    build: "production",
    strictModeDevReplay: false,
    unverifiedEngines: ["WebKitGTK", "WebView2"],
  };
}

export type ScenarioReport = {
  name: string;
  phase: "cold" | "hot";
  interaction: SampleSummary;
  visible?: SampleSummary;
  scriptDurationDelta?: SampleSummary | { unmeasured: true };
  targetMs?: number;
  note?: string;
};

export type LoadedCounts = {
  articlesListed: number;
  articlesConfigured: number;
  chatBubbles: number;
  historyMessagesConfigured: number;
  readerBlocks: number;
  readerImages: number;
  readerImagesDecoded: number;
  readerBlocksConfigured: number;
  readerImagesConfigured: number;
};

export type StreamTiming = {
  emitted: number;
  observed: number;
  matched: number;
  missed: number;
};

export type PerfReport = {
  scene: string;
  spec: SceneSpec;
  targets: typeof PERF_TARGETS;
  machine: MachineInfo;
  metricMeanings: typeof METRIC_MEANINGS;
  notes: string[];
  coldStart: {
    firstArticleMs: number;
    source: "mutation-observer";
    phase: "cold";
    timeOrigin: "document-performance-now";
  };
  loaded: LoadedCounts;
  stream: StreamTiming;
  scenarios: Record<string, ScenarioReport>;
  overruns: TargetCheck[];
  ipc: {
    mock: true;
    rust: false;
    methods: Array<{
      method: string;
      count: number;
      requestBytes: number;
      responseBytes: number;
      p50LatencyMs: number | null;
      p95LatencyMs: number | null;
    }>;
  };
  cdp: {
    scriptDurationTotalMs: number | null;
    jsHeapUsedSize: number | null;
    nodes: number | null;
    jsP95Ms: null;
    jsP95Status: "unmeasured";
  };
  abortedRequests: Array<{ host: string; resourceType: string; count: number }>;
  pageerrorCount: number;
  mediaProtocol: "loopback-http-fixture";
};

export const PERF_REPORT_NOTES = [
  "交互时间为页面内 dispatch → DOM 可见 / 下一帧的代理，不是真实 INP。",
  "不要把 Playwright 跨进程动作延迟当成纯 JS 时间。",
  "IPC 字节是完整 JSON 参数的 UTF-8 长度，不是 Object.keys(args)。日志不存参数内容。",
  "scriptDuration 是测量窗口（含准备 / 关闭等）内的 CDP 累积增量，只当窗口工作量；jsP95 标未测，不与 8ms 比较。",
  "流式时延按唯一 textLength 相等 + 时间顺序匹配 emit→渲染；未匹配记 missed。结束前等待 chat-status 消失且落库历史可见。",
  "viewSwitch* 只保留三次单步，各自对照 100ms。不派生 cycle。",
  "coldStart 由页内 MutationObserver 在首篇 article 可见时记录，不是仅节点插入。",
  "截图发生在 audit / CDP / 场景统计快照之后，不计入最终测量。",
  "IPC 字节与 latency 来自页内严格模拟宿主，不是真实 Rust / 自定义协议。",
  "图片 href 为 mock 宿主提供的回环 HTTP，Playwright route 返回本地 SVG/PNG；未测试 rrss:// 媒体协议。",
  "p95 起始目标仅用于报告超出，不使当前已知缺陷导致默认 suite 失败。",
  "生产构建；React production 下 StrictMode 不会开发态双调用。未验 WebKitGTK / WebView2。",
];
