import { act, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createReactHarness } from "../test/react";
import { ModelField } from "./ModelField";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });

function Fixture() {
  const [value, setValue] = useState("model-b");
  const [options, setOptions] = useState(["model-a", "model-b", "model-c"]);
  return <>
    <ModelField label="主模型" value={value} onChange={setValue} placeholder="模型名称" options={options} />
    <button onClick={() => setOptions([])}>清空候选</button>
    <button>下一控件</button>
  </>;
}

function key(target: HTMLElement, key: string) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  act(() => { target.dispatchEvent(event); });
  return event;
}

describe("ModelField", () => {
  it("opens from the input, selects with the keyboard and restores its opener", () => {
    const { host } = harness.mount(<Fixture />);
    const input = host.querySelector<HTMLInputElement>(".model-text-input")!;
    act(() => input.focus());
    key(input, "ArrowDown");
    const search = host.querySelector<HTMLInputElement>('[role=combobox]')!;
    expect(document.activeElement).toBe(search);
    const selected = host.querySelector('[role=option][aria-selected=true]')!;
    expect(search.getAttribute("aria-activedescendant")).toBe(selected.id);
    key(search, "ArrowDown");
    key(search, "Enter");
    expect(input.value).toBe("model-c");
    expect(host.querySelector('[role=listbox]')).toBeNull();
    expect(document.activeElement).toBe(input);
  });

  it("closes on focus departure without stealing focus and ignores empty results", () => {
    const { host } = harness.mount(<Fixture />);
    const trigger = host.querySelector<HTMLButtonElement>(".model-dropdown-trigger")!;
    act(() => trigger.click());
    const search = host.querySelector<HTMLInputElement>('[role=combobox]')!;
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(search, "missing");
      search.dispatchEvent(new Event("input", { bubbles: true }));
    });
    key(search, "ArrowDown");
    key(search, "Enter");
    expect(host.querySelectorAll('[role=option]')).toHaveLength(0);
    expect(search.hasAttribute("aria-activedescendant")).toBe(false);
    expect(host.querySelector<HTMLInputElement>(".model-text-input")!.value).toBe("model-b");
    const next = [...host.querySelectorAll("button")].find(b => b.textContent === "下一控件")!;
    act(() => next.focus());
    expect(host.querySelector('[role=listbox]')).toBeNull();
    expect(document.activeElement).toBe(next);
    act(() => trigger.click());
    const clear = [...host.querySelectorAll("button")].find(b => b.textContent === "清空候选")!;
    act(() => clear.click());
    expect(host.querySelector(".model-dropdown-menu")).toBeNull();
  });

  it("consumes Escape before the settings overlay and restores the trigger", () => {
    const { host } = harness.mount(<Fixture />);
    const trigger = host.querySelector<HTMLButtonElement>(".model-dropdown-trigger")!;
    act(() => trigger.click());
    const outerKey = vi.fn();
    window.addEventListener("keydown", outerKey);
    try {
      key(host.querySelector<HTMLInputElement>('[role=combobox]')!, "Escape");
      expect(outerKey).not.toHaveBeenCalled();
      expect(document.activeElement).toBe(trigger);
      expect(host.querySelector('[role=listbox]')).toBeNull();
    } finally { window.removeEventListener("keydown", outerKey); }
  });
  it("does not select a model while the input method is composing", () => {
    const { host } = harness.mount(<Fixture />);
    act(() => host.querySelector<HTMLButtonElement>(".model-dropdown-trigger")!.click());
    const search = host.querySelector<HTMLInputElement>('[role="combobox"]')!;
    const event = new KeyboardEvent("keydown", { key: "Enter", isComposing: true, bubbles: true, cancelable: true });
    act(() => search.dispatchEvent(event));
    expect(event.defaultPrevented).toBe(false);
    expect(host.querySelector('[role="listbox"]')).not.toBeNull();
    expect(host.querySelector<HTMLInputElement>(".model-text-input")!.value).toBe("model-b");
  });

});
