export type StreamEmitMeta = {
  seq: number;
  emittedAt: number;
  textLength: number;
};

export type RrssPerfBridge = {
  listedCount: () => number;
  hasMore: () => boolean;
  streamLog: () => StreamEmitMeta[];
  streamDone: () => boolean;
  audit: () => {
    unexpected: string[];
    ipc: Array<{ method: string; requestBytes: number; responseBytes: number; durationMs: number }>;
    streamEmits: number;
    listedCount: number;
  };
  dispose: () => void;
};

declare global {
  interface Window {
    __rrssPerf?: RrssPerfBridge;
    __rrssPerfCold?: { firstArticleMs: number | null; source: "mutation-observer" };
  }
}

export {};
