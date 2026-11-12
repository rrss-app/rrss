import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { ConfirmBar } from "./ConfirmBar";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

describe("ConfirmBar", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("exposes alertdialog semantics and a cancel control", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onOk = vi.fn();
    const onCancel = vi.fn();
    act(() => {
      root.render(<ConfirmBar title="删除订阅" message="此操作不可恢复" onOk={onOk} onCancel={onCancel} />);
    });
    const dialog = host.querySelector("[role=alertdialog]");
    expect(dialog?.classList.contains("confirm-panel")).toBe(true);
    expect(dialog?.getAttribute("aria-modal")).toBe("true");
    expect(dialog?.getAttribute("aria-labelledby")).toBe("confirm-title");
    expect(host.querySelector("#confirm-title")?.textContent).toBe("删除订阅");
    const cancel = host.querySelector("[data-confirm-cancel]") as HTMLButtonElement;
    expect(cancel.textContent).toBe("取消");
    act(() => {
      cancel.click();
    });
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onOk).not.toHaveBeenCalled();
    act(() => root.unmount());
    host.remove();
  });
});
