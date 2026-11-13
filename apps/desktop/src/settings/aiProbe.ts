import { useEffect, useRef, useState } from "react";
import { i18n } from "../i18n";

/** 测通 / 取模型列表的前端等待上限。现有 IPC 无取消，停止只是不再等待，后台请求可能继续。 */
export const AI_PROBE_SLOW_MS = 10_000;
export const AI_PROBE_CAP_MS = 35_000;

export type ProbeWait = { key: string; seconds: number } | null;
/** `note` 为空表示被新的请求取代，调用方不必提示。 */
export type ProbeOutcome<T> = { done: true; value: T } | { done: false; note: string };

const stoppedNote = () => i18n.t("probe.stopped", { ns: "settingsModels" });
const timeoutNote = () => i18n.t("probe.timeout", { ns: "settingsModels", count: AI_PROBE_CAP_MS / 1000 });

/**
 * 同一时刻只等一个请求；新请求、停止、超时或卸载都会让旧请求的结果作废。
 * 失败原样抛出，由调用方决定文案。
 */
export function useProbe() {
  const [wait, setWait] = useState<ProbeWait>(null);
  const abandon = useRef<((note: string) => void) | null>(null);

  useEffect(() => () => abandon.current?.(""), []);
  const waiting = wait !== null;
  useEffect(() => {
    if (!waiting) return;
    const tick = window.setInterval(() => setWait((w) => (w ? { ...w, seconds: w.seconds + 1 } : w)), 1_000);
    return () => window.clearInterval(tick);
  }, [waiting]);

  /** `work` 收到 `live()`：停止等待后不要再发起后续请求。 */
  const run = <T,>(key: string, work: (live: () => boolean) => Promise<T>): Promise<ProbeOutcome<T>> => {
    abandon.current?.("");
    setWait({ key, seconds: 0 });
    return new Promise<ProbeOutcome<T>>((resolve, reject) => {
      let live = true;
      const finish = (fn: () => void) => {
        if (!live) return;
        live = false;
        window.clearTimeout(cap);
        if (abandon.current === stop) {
          abandon.current = null;
          setWait(null);
        }
        fn();
      };
      const stop = (note: string) => finish(() => resolve({ done: false, note }));
      const cap = window.setTimeout(() => stop(timeoutNote()), AI_PROBE_CAP_MS);
      abandon.current = stop;
      work(() => live).then((value) => finish(() => resolve({ done: true, value })), (error) => finish(() => reject(error)));
    });
  };

  return { wait, run, stop: () => abandon.current?.(stoppedNote()) };
}

export function probeWaitLabel(wait: NonNullable<ProbeWait>) {
  return i18n.t(wait.seconds * 1000 >= AI_PROBE_SLOW_MS ? "probe.waitedSlow" : "probe.waited", { ns: "settingsModels", count: wait.seconds });
}
