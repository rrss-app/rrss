import { StrictMode, useRef, useState, type ReactNode } from "react";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { getFocusableElements, isFocusable, useModalFocus, type ModalLayerId } from "./useModalFocus";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let cleanup = () => {};
afterEach(() => {
  cleanup();
  document.body.replaceChildren();
});

function mount(node: ReactNode) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  act(() => {
    root.render(node);
  });
  cleanup = () => {
    act(() => root.unmount());
    host.remove();
  };
  return host;
}

function tab(root: HTMLElement, shift = false) {
  act(() => {
    root.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey: shift, bubbles: true, cancelable: true }));
  });
}

function Fixture({
  layers,
  activeId,
  suspended = false,
  triggerGone = false,
}: {
  layers: ModalLayerId[];
  activeId: ModalLayerId | null;
  suspended?: boolean;
  triggerGone?: boolean;
}) {
  const settingsRef = useRef<HTMLDivElement>(null);
  const confirmRef = useRef<HTMLDivElement>(null);
  useModalFocus({
    layers,
    activeId,
    suspended,
    roots: { settings: settingsRef, confirm: confirmRef },
    initialFocus: { settings: "[data-settings-back]", confirm: "[data-confirm-cancel]" },
  });
  return (
    <div>
      <button type="button" data-open-settings>
        打开设置
      </button>
      <button type="button" data-outside>
        外面
      </button>
      {layers.includes("settings") ? (
        <div ref={settingsRef} role="dialog" aria-modal="true">
          <button type="button" data-settings-back>
            返回
          </button>
          <button type="button">订阅</button>
          {triggerGone ? null : (
            <button type="button" data-delete>
              删除
            </button>
          )}
          <button type="button" disabled>
            禁用
          </button>
          <button type="button" hidden>
            隐藏
          </button>
          <button type="button" style={{ display: "none" }}>
            无显示
          </button>
          <details>
            <summary>折叠</summary>
            <button type="button" data-inside-details>
              折叠内
            </button>
          </details>
        </div>
      ) : null}
      <div className="command-palette-stage">
        <input data-search placeholder="搜索" />
      </div>
      {layers.includes("confirm") ? (
        <div ref={confirmRef} role="alertdialog" aria-modal="true">
          <button type="button" data-confirm-ok>
            确认
          </button>
          <button type="button" data-confirm-cancel>
            取消
          </button>
        </div>
      ) : null}
    </div>
  );
}

describe("getFocusableElements", () => {
  it("skips disabled, hidden, closed details, and overlay-dismiss", () => {
    const root = document.createElement("div");
    root.innerHTML = `
      <button type="button">可见</button>
      <button type="button" disabled>禁用</button>
      <button type="button" hidden>隐藏</button>
      <button type="button" class="overlay-dismiss">关闭</button>
      <input type="hidden" value="x" />
      <details>
        <summary>摘要</summary>
        <button type="button">里面</button>
      </details>
    `;
    document.body.append(root);
    try {
      const labels = getFocusableElements(root).map((el) => el.textContent?.trim());
      expect(labels).toEqual(["可见", "摘要"]);
      const inner = root.querySelector("details button") as HTMLElement;
      expect(isFocusable(inner)).toBe(false);
      root.querySelector("details")!.open = true;
      expect(isFocusable(inner)).toBe(true);
    } finally {
      root.remove();
    }
  });
});

describe("useModalFocus", () => {
  it("moves focus to the initial control and restores the opener", async () => {
    const opener = document.createElement("button");
    opener.textContent = "外部";
    document.body.append(opener);
    opener.focus();
    const host = mount(<Fixture layers={["settings"]} activeId="settings" />);
    expect(document.activeElement).toBe(host.querySelector("[data-settings-back]"));
    cleanup();
    cleanup = () => {};
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("wraps Tab and Shift+Tab at the dialog edges", () => {
    const host = mount(<Fixture layers={["settings"]} activeId="settings" />);
    const root = host.querySelector("[role=dialog]") as HTMLElement;
    const back = host.querySelector("[data-settings-back]") as HTMLElement;
    const subscribe = [...host.querySelectorAll("button")].find((b) => b.textContent === "订阅")!;
    const summary = host.querySelector("summary") as HTMLElement;
    expect(document.activeElement).toBe(back);
    tab(root);
    expect(document.activeElement).toBe(subscribe);
    summary.focus();
    tab(root);
    expect(document.activeElement).toBe(back);
    tab(root, true);
    expect(document.activeElement).toBe(summary);
  });

  it("recovers focus when the active action becomes disabled", async () => {
    const host = mount(<Fixture layers={["settings"]} activeId="settings" />);
    const action = host.querySelector<HTMLButtonElement>("[data-delete]")!;
    action.focus();
    await act(async () => {
      action.disabled = true;
      await Promise.resolve();
    });
    expect(document.activeElement).toBe(host.querySelector("[data-settings-back]"));
  });

  it("recognizes WebKitGTK Shift+Tab with an unidentified key", () => {
    const host = mount(<Fixture layers={["settings"]} activeId="settings" />);
    act(() => {
      host.querySelector("[data-settings-back]")!.dispatchEvent(new KeyboardEvent("keydown", {
        key: "Unidentified", code: "Tab", shiftKey: true, bubbles: true, cancelable: true,
      }));
    });
    expect(document.activeElement).toBe(host.querySelector("summary"));
  });

  it("does not steal focus from search while suspended, then resumes", () => {
    function SearchCase() {
      const [suspended, setSuspended] = useState(true);
      (SearchCase as unknown as { setSuspended: (v: boolean) => void }).setSuspended = setSuspended;
      return <Fixture layers={["settings"]} activeId="settings" suspended={suspended} />;
    }
    const host = mount(<SearchCase />);
    const search = host.querySelector("[data-search]") as HTMLInputElement;
    const outside = host.querySelector("[data-outside]") as HTMLButtonElement;
    search.focus();
    expect(document.activeElement).toBe(search);
    act(() => {
      outside.focus();
    });
    expect(document.activeElement).toBe(outside);
    act(() => {
      (SearchCase as unknown as { setSuspended: (v: boolean) => void }).setSuspended(false);
    });
    expect(document.activeElement).toBe(host.querySelector("[data-settings-back]"));
  });

  it("restores last focus after a higher overlay suspends the trap", () => {
    function ResumeCase() {
      const [activeId, setActiveId] = useState<ModalLayerId | null>("settings");
      (ResumeCase as unknown as { setActiveId: typeof setActiveId }).setActiveId = setActiveId;
      return <Fixture layers={["settings"]} activeId={activeId} />;
    }
    const host = mount(<ResumeCase />);
    const subscribe = [...host.querySelectorAll("button")].find((b) => b.textContent === "订阅")!;
    act(() => {
      subscribe.focus();
    });
    act(() => {
      (ResumeCase as unknown as { setActiveId: (id: ModalLayerId | null) => void }).setActiveId(null);
    });
    const outside = host.querySelector("[data-outside]") as HTMLButtonElement;
    act(() => {
      outside.focus();
    });
    act(() => {
      (ResumeCase as unknown as { setActiveId: (id: ModalLayerId | null) => void }).setActiveId("settings");
    });
    expect(document.activeElement).toBe(subscribe);
  });

  it("restores the trigger after nested confirm cancel", () => {
    function Nested() {
      const [layers, setLayers] = useState<ModalLayerId[]>(["settings"]);
      (Nested as unknown as { setLayers: typeof setLayers }).setLayers = setLayers;
      return <Fixture layers={layers} activeId={layers.at(-1) ?? null} />;
    }
    const host = mount(<Nested />);
    const del = host.querySelector("[data-delete]") as HTMLButtonElement;
    act(() => {
      del.focus();
    });
    act(() => {
      (Nested as unknown as { setLayers: (layers: ModalLayerId[]) => void }).setLayers(["settings", "confirm"]);
    });
    expect(document.activeElement).toBe(host.querySelector("[data-confirm-cancel]"));
    act(() => {
      (Nested as unknown as { setLayers: (layers: ModalLayerId[]) => void }).setLayers(["settings"]);
    });
    expect(document.activeElement).toBe(del);
  });

  it("falls back when the trigger unmounts", () => {
    function Gone() {
      const [layers, setLayers] = useState<ModalLayerId[]>(["settings", "confirm"]);
      const [gone, setGone] = useState(false);
      (Gone as unknown as { closeAndRemove: () => void }).closeAndRemove = () => {
        setGone(true);
        setLayers(["settings"]);
      };
      return <Fixture layers={layers} activeId={layers.at(-1) ?? null} triggerGone={gone} />;
    }
    const host = mount(<Gone />);
    expect(document.activeElement).toBe(host.querySelector("[data-confirm-cancel]"));
    act(() => {
      (Gone as unknown as { closeAndRemove: () => void }).closeAndRemove();
    });
    expect(host.querySelector("[data-delete]")).toBeNull();
    expect(document.activeElement).toBe(host.querySelector("[data-settings-back]"));
  });

  it("keeps the initial control under StrictMode remounts", () => {
    const opener = document.createElement("button");
    opener.textContent = "外部";
    document.body.append(opener);
    opener.focus();
    const host = mount(
      <StrictMode>
        <Fixture layers={["settings"]} activeId="settings" />
      </StrictMode>,
    );
    expect(document.activeElement).toBe(host.querySelector("[data-settings-back]"));
    opener.remove();
  });

  it("does not pull focus out of a reader selection menu while settings is covered", () => {
    const host = mount(<Fixture layers={["settings"]} activeId={null} />);
    const menu = document.createElement("div");
    menu.className = "sel-menu";
    const item = document.createElement("button");
    item.textContent = "引用";
    menu.append(item);
    document.body.append(menu);
    act(() => {
      item.focus();
    });
    expect(document.activeElement).toBe(item);
    expect(host.querySelector("[data-settings-back]")).not.toBe(document.activeElement);
    menu.remove();
  });
});
