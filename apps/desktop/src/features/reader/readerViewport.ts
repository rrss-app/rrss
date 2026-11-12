import type { TranslationViewport } from "../../bindings";

const BLOCKS = "[data-reader-document] > [data-block-id]";

/** 只观察原块边界，双语的一对仍算一个块。滚动时只测附近候选，首次 / 结构变化才扫描节点。 */
export function observeReaderViewport(body: HTMLElement, changed: (viewport: TranslationViewport) => void) {
  let nodes = new Set<HTMLElement>(), candidates = new Set<HTMLElement>();
  let observer: IntersectionObserver | undefined, frame: number | undefined, disposed = false;
  let height = -1, structureChanged = true;
  const measure = (all = false) => {
    frame = undefined;
    if (disposed || !body.isConnected || body.clientHeight === 0) return;
    if (structureChanged) {
      const next = new Set(body.querySelectorAll<HTMLElement>(BLOCKS));
      for (const node of nodes) if (!next.has(node)) { observer?.unobserve(node); candidates.delete(node); }
      for (const node of next) if (!nodes.has(node)) { candidates.add(node); observer?.observe(node); }
      nodes = next; structureChanged = false;
    }
    if (height !== body.clientHeight) {
      height = body.clientHeight;
      observer?.disconnect();
      candidates = new Set(nodes);
      if (typeof IntersectionObserver !== "undefined") {
        observer = new IntersectionObserver(entries => {
          if (disposed) return;
          for (const entry of entries) {
            if (entry.isIntersecting) candidates.add(entry.target as HTMLElement);
            else candidates.delete(entry.target as HTMLElement);
          }
          schedule();
        }, { root: body, rootMargin: `${height}px 0px`, threshold: 0 });
        for (const node of nodes) observer.observe(node);
      }
    }
    const top = body.getBoundingClientRect().top + body.clientTop, bottom = top + height;
    const visible: number[] = [], nearby: number[] = [];
    for (const node of all ? nodes : candidates) {
      const rect = node.getBoundingClientRect(), id = Number(node.dataset.blockId);
      if (!Number.isSafeInteger(id) || id < 0 || rect.height <= 0) continue;
      if (rect.bottom > top && rect.top < bottom) visible.push(id);
      else if (rect.bottom > top - height && rect.top < bottom + height) nearby.push(id);
    }
    changed({ visible: visible.sort((a, b) => a - b), nearby: nearby.sort((a, b) => a - b) });
  };
  const schedule = () => { if (!disposed && frame === undefined) frame = requestAnimationFrame(() => measure()); };
  const mutation = new MutationObserver(() => { structureChanged = true; schedule(); });
  mutation.observe(body, { childList: true, subtree: true });
  const resize = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(schedule);
  resize?.observe(body);
  if (body.firstElementChild) resize?.observe(body.firstElementChild);
  body.addEventListener("scroll", schedule, { passive: true });
  body.addEventListener("load", schedule, true);
  measure();
  return {
    // 开始翻译时读取此刻视口；不等异步观察器，快跳后首次启动也能带准确块。
    read() { if (frame !== undefined) cancelAnimationFrame(frame); measure(true); },
    dispose() {
      disposed = true;
      if (frame !== undefined) cancelAnimationFrame(frame);
      observer?.disconnect(); mutation.disconnect(); resize?.disconnect();
      body.removeEventListener("scroll", schedule); body.removeEventListener("load", schedule, true);
    },
  };
}
