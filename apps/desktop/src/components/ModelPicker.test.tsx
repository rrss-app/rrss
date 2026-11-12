import { act } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { createReactHarness } from "../test/react";
import { click, pressKey } from "../test/fixtures/shell";
import { ModelPicker } from "./ModelPicker";

const harness = createReactHarness();
afterEach(() => harness.cleanup());
it("gives composition priority and consumes Escape even after focus moves outside the model menu", () => {
  const { host } = harness.mount(<><ModelPicker label="模型" value="a" items={[{ key: "a", label: "a" }, { key: "b", label: "b" }]} onPick={() => {}} display={{ text: "a", title: "a" }} /><textarea /></>);
  const trigger = host.querySelector("button")!;
  click(trigger);
  const input = host.querySelector(".model-picker-search input")!;
  pressKey("Escape", { isComposing: true }, input);
  pressKey("Escape", { keyCode: 229 }, input);
  expect(host.querySelector(".model-picker-menu")).not.toBeNull();
  host.setAttribute("inert", "");
  pressKey("Escape");
  expect(host.querySelector(".model-picker-menu")).not.toBeNull();
  host.removeAttribute("inert");
  act(() => host.querySelector("textarea")!.focus());
  const shellEscape = vi.fn(); window.addEventListener("keydown", shellEscape);
  try {
    pressKey("Escape", {}, document.activeElement!);
    expect(host.querySelector(".model-picker-menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(shellEscape).not.toHaveBeenCalled();
  } finally { window.removeEventListener("keydown", shellEscape); }
});
