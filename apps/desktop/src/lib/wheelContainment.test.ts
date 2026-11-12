import { afterEach, describe, expect, it, vi } from "vitest";
import { containWheel } from "./wheelContainment";

afterEach(() => { document.body.replaceChildren(); vi.unstubAllGlobals(); });

/** body 可滚（1000 / 200）；inner 的内容高度可调，默认可滚（500 / 100）。 */
function fixture(height = 500) {
  const shell = document.createElement("div");
  const body = document.createElement("div");
  const inner = document.createElement("div");
  body.style.overflowY = inner.style.overflowY = "auto";
  body.append(inner); shell.append(body); document.body.append(shell);
  Object.defineProperties(inner, { scrollHeight: { value: height }, clientHeight: { value: 100 } });
  Object.defineProperties(body, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 } });
  shell.addEventListener("wheel", containWheel, { passive: false });
  return { shell, body, inner };
}

function wheel(target: HTMLElement, deltaY: number, ctrlKey = false) {
  const event = new WheelEvent("wheel", { deltaY, ctrlKey, bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

describe("wheel containment", () => {
  it("a box that can scroll keeps the wheel at both edges and clamps the last step", () => {
    const { body, inner } = fixture();
    expect(wheel(inner, -60).defaultPrevented).toBe(true);
    inner.scrollTop = 380;
    expect(wheel(inner, 80).defaultPrevented).toBe(true);
    expect(inner.scrollTop).toBe(400);
    expect(wheel(inner, 80).defaultPrevented).toBe(true);
    expect(body.scrollTop).toBe(0);
  });

  it("glides the last step to the edge instead of jumping, unless motion is reduced", () => {
    const { inner } = fixture();
    const calls: ScrollToOptions[] = [];
    inner.scrollTo = ((options: ScrollToOptions) => { calls.push(options); }) as typeof inner.scrollTo;
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: false, media: query }));
    inner.scrollTop = 380;
    expect(wheel(inner, 80).defaultPrevented).toBe(true);
    expect(calls).toEqual([{ top: 400, behavior: "smooth" }]);
    // 已在边界：只拦住，不再发起滚动。
    inner.scrollTop = 400;
    expect(wheel(inner, 80).defaultPrevented).toBe(true);
    expect(calls).toHaveLength(1);
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("reduce"), media: query }));
    inner.scrollTop = 20;
    expect(wheel(inner, -60).defaultPrevented).toBe(true);
    expect(inner.scrollTop).toBe(0);
    expect(calls).toHaveLength(1);
  });

  it("leaves normal scrolling and zoom gestures to the browser", () => {
    const { inner } = fixture();
    inner.scrollTop = 100;
    expect(wheel(inner, 40).defaultPrevented).toBe(false);
    expect(wheel(inner, -400, true).defaultPrevented).toBe(false);
  });

  it("a box with nothing to scroll passes the wheel to the outer scroller; boundaries still stop it", () => {
    const { body, inner } = fixture(80);
    // 短代码块 / 短列表：不拦，外层 body 照常滚。
    expect(wheel(inner, 60).defaultPrevented).toBe(false);
    expect(wheel(body, 60).defaultPrevented).toBe(false);
    const menu = document.createElement("div");
    menu.dataset.scrollBoundary = "";
    body.append(menu);
    expect(wheel(menu, 60).defaultPrevented).toBe(true);
  });
});
