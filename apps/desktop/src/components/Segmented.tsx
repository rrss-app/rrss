import { useLayoutEffect, useRef, type ComponentPropsWithoutRef } from "react";

/** 只画分段控件的滑块：选择、语义和键盘行为都由调用方负责。 */
export function Segmented({ children, ...props }: ComponentPropsWithoutRef<"div">) {
  const root = useRef<HTMLDivElement>(null);
  const thumb = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const group = root.current!;
    const marker = thumb.current!;
    // 只有换了选中项才滑过去；首次定位、从隐藏（保活栏目）回来、尺寸变化都直接到位，不从左边滑进来。
    let shown: Element | null = null;
    const measure = () => {
      const selected = group.querySelector<HTMLButtonElement>(':scope > button:is([data-on="1"], [aria-pressed="true"], [aria-checked="true"])');
      marker.hidden = !selected || !selected.offsetWidth;
      if (!selected || marker.hidden) { shown = null; return; }
      const slide = shown !== null && shown !== selected;
      shown = selected;
      if (!slide) marker.style.transition = "none";
      marker.style.width = `${selected.offsetWidth}px`;
      marker.style.height = `${selected.offsetHeight}px`;
      marker.style.transform = `translate(${selected.offsetLeft}px, ${selected.offsetTop}px)`;
      if (!slide) { void marker.offsetWidth; marker.style.transition = ""; }
    };
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    const observe = () => {
      resize?.disconnect();
      resize?.observe(group);
      for (const button of group.querySelectorAll(":scope > button")) resize?.observe(button);
      measure();
    };
    const mutation = new MutationObserver(observe);
    mutation.observe(group, { subtree: true, childList: true, attributes: true, attributeFilter: ["data-on", "aria-pressed", "aria-checked"] });
    observe();
    if (!resize) window.addEventListener("resize", measure);
    return () => { resize?.disconnect(); mutation.disconnect(); window.removeEventListener("resize", measure); };
  }, []);
  return <div {...props} ref={root}><span className="segment-thumb" aria-hidden="true" ref={thumb} hidden />{children}</div>;
}
