import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { Toast } from "./Toast";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

function mount(node: ReactNode) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(node);
  });
  return { host, unmount: () => { act(() => root.unmount()); host.remove(); } };
}

describe("Toast", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("shows a short message and can be dismissed", () => {
    let closed = false;
    const { host, unmount } = mount(
      <Toast message="摘要生成失败：模型没有返回可用的 JSON，请换模型或重试" onDismiss={() => { closed = true; }} holdMs={60_000} />,
    );
    const el = host.querySelector(".toast");
    expect(el?.textContent).toContain("摘要生成失败");
    expect(el?.textContent).not.toContain("Armbian");
    act(() => {
      (host.querySelector(".toast-dismiss") as HTMLButtonElement).click();
    });
    expect(closed).toBe(true);
    unmount();
  });
});
