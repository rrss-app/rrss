import { act, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createReactHarness } from "../test/react";
import { Stepper } from "./Stepper";

const harness = createReactHarness();
afterEach(() => harness.cleanup());

describe("Stepper", () => {
  it("edits with arrows or typing and commits once on blur or pointer release", async () => {
    const clock = harness.useClock();
    const save = vi.fn();
    function Editor() {
      const [value, setValue] = useState("-10");
      return <Stepper label="隐藏阈值" value={value} onChange={setValue} onCommit={save} />;
    }
    const { host } = harness.mount(<Editor />);
    const input = host.querySelector("input")!;
    for (const key of ["ArrowUp", "ArrowUp", "ArrowDown"]) {
      act(() => input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
    }
    expect(input.value).toBe("-9");
    expect(save).not.toHaveBeenCalled();
    act(() => input.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    expect(save.mock.calls).toEqual([["-9"]]);
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "-25");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(save).toHaveBeenCalledTimes(1);
    act(() => input.dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    expect(save.mock.calls).toEqual([["-9"], ["-25"]]);
    const plus = host.querySelector<HTMLButtonElement>('[aria-label="增加隐藏阈值"]')!;
    plus.setPointerCapture = vi.fn();
    const down = new Event("pointerdown", { bubbles: true, cancelable: true });
    Object.defineProperties(down, { button: { value: 0 }, pointerId: { value: 1 } });
    act(() => plus.dispatchEvent(down));
    await clock.advance(560);
    expect(input.value).toBe("-21");
    expect(save).toHaveBeenCalledTimes(2);
    act(() => {
      plus.dispatchEvent(new Event("pointerup", { bubbles: true }));
      plus.dispatchEvent(new Event("lostpointercapture", { bubbles: true }));
      plus.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
    });
    expect(save.mock.calls).toEqual([["-9"], ["-25"], ["-21"]]);
    await clock.advance(500);
    expect(input.value).toBe("-21");
  });
});
