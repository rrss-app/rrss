import { api } from "./api";
import { isTauri } from "./preview";
import type { ClientLogLevel } from "../bindings";

const WINDOW_MS = 60_000;
const LIMIT = 20;
let started = false;
let since = 0;
let count = 0;
const recent = new Map<string, number>();

function details(error: unknown): { message: string; stack?: string } {
  // Never stringify arbitrary rejection objects: they may contain documents,
  // requests, credentials, or getters that throw again while reporting.
  try {
    const bounded = (text: string, limit: number) => text.length > limit ? "<oversized field omitted>" : text;
    if (error instanceof Error) return { message: bounded(error.message, 4096), stack: error.stack ? bounded(error.stack, 8192) : undefined };
    if (error && typeof error === "object" && "code" in error && typeof error.code === "string"
      && /^[a-z][a-z0-9_]{0,127}$/.test(error.code)) {
      // Legacy detail is display text and can contain model output. The host
      // keeps its diagnostic projection; this bridge reports only the code.
      return { message: `UI error ${error.code}` };
    }
    return { message: typeof error === "string" ? bounded(error, 4096) : "non-error rejection" };
  } catch { return { message: "unreadable error" }; }
}

export function reportClientError(scope: string, error: unknown, level: ClientLogLevel = "error"): void {
  if (!started) return;
  const now = performance.now();
  if (now - since >= WINDOW_MS) { since = now; count = 0; }
  for (const [key, at] of recent) if (now - at >= WINDOW_MS) recent.delete(key);
  const { message, stack } = details(error);
  const key = message;
  if (recent.has(key) || count >= LIMIT) return;
  recent.set(key, now);
  count++;
  // The bridge can reject or throw synchronously during teardown. Reporting
  // failures are terminal and must not become another unhandled rejection.
  try { void api.logClient(level, scope.slice(0, 128), message, stack).catch(() => {}); }
  catch { /* best effort */ }
}

/** Installed only in main.tsx. Component tests stay silent unless explicitly enabled. */
export function startClientLog(): () => void {
  if (started || !isTauri()) return () => {};
  started = true;
  since = performance.now();
  count = 0;
  recent.clear();
  const onError = (event: ErrorEvent) => reportClientError("window", event.error ?? event.message);
  const onRejection = (event: PromiseRejectionEvent) => reportClientError("unhandledrejection", event.reason);
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);
  return () => {
    started = false;
    recent.clear();
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onRejection);
  };
}
