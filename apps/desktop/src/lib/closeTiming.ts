import { api } from "./api";

let clicked: number | undefined;
let received: number | undefined;
let dispatchToEvent: number | undefined;

export function markCloseClick() {
  clicked = performance.now();
  received = undefined;
}

export function markCloseReceived(hostReceivedAt?: number | null) {
  received = performance.now();
  dispatchToEvent = typeof hostReceivedAt === "number" ? Math.max(0, Date.now() - hostReceivedAt) : undefined;
  performance.clearMeasures("rrss.close.event_to_paint");
  performance.clearMeasures("rrss.close.click_to_paint");
  // Native title-bar/keyboard closes have no frontend click. Discard old clicks.
  if (clicked !== undefined && received - clicked > 30_000) clicked = undefined;
}

export function measureClosePaint() {
  if (received === undefined) return;
  const end = performance.now();
  const eventToPaint = end - received;
  const clickToPaint = clicked === undefined ? undefined : end - clicked;
  const sample = { event_to_paint_ms: eventToPaint, click_to_paint_ms: clickToPaint, host_to_event_ms: dispatchToEvent, host_to_paint_ms: dispatchToEvent === undefined ? undefined : dispatchToEvent + eventToPaint };
  performance.measure("rrss.close.event_to_paint", { start: received, end, detail: sample });
  if (clicked !== undefined) performance.measure("rrss.close.click_to_paint", { start: clicked, end });
  if (Math.max(clickToPaint ?? 0, sample.host_to_paint_ms ?? eventToPaint) >= 100) {
    void api.logClient("warn", "window_close_latency", JSON.stringify(sample)).catch(() => {});
  }
  clicked = undefined;
  received = undefined;
}
