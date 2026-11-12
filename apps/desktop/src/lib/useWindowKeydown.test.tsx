import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createReactHarness } from "../test/react";
import { useWindowKeydown } from "./useWindowKeydown";

function Probe({ enabled, onKey }: { enabled: boolean; onKey: (key: string) => void }) {
  useWindowKeydown(enabled, (e) => onKey(e.key));
  return null;
}

describe("useWindowKeydown", () => {
  const harness = createReactHarness();
  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
  });

  it("binds only while enabled, reads the latest handler, and unbinds on unmount", () => {
    const add = vi.spyOn(window, "addEventListener");
    const remove = vi.spyOn(window, "removeEventListener");
    const keydownAdds = () => add.mock.calls.filter((c) => c[0] === "keydown").length;
    const keydownRemoves = () => remove.mock.calls.filter((c) => c[0] === "keydown").length;
    const seen: string[] = [];
    const { unmount } = harness.mount(<Probe enabled={false} onKey={(k) => seen.push(`a:${k}`)} />);
    expect(keydownAdds()).toBe(0);

    act(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "x" })));
    expect(seen).toEqual([]);

    harness.mount(<Probe enabled={true} onKey={(k) => seen.push(`b:${k}`)} />);
    expect(keydownAdds()).toBe(1);
    act(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "y" })));
    expect(seen).toEqual(["b:y"]);

    const second = harness.mount(<Probe enabled={true} onKey={(k) => seen.push(`c:${k}`)} />);
    expect(keydownAdds()).toBe(2);
    second.unmount();
    expect(keydownRemoves()).toBe(1);
    act(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "z" })));
    expect(seen).toEqual(["b:y", "b:z"]);
    unmount();
    expect(keydownRemoves()).toBe(1);
  });
});
