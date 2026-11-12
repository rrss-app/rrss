import { describe, expect, it, vi } from "vitest";
import { createExposureGate } from "./exposureGate";

describe("exposureGate", () => {
  it("exposes at exactly one second, but not at 0.99 seconds", () => {
    const onExpose = vi.fn();
    const gate = createExposureGate({ onExpose });
    gate.update(1, 0, 0.5, 100);
    gate.tick(1090);
    expect(onExpose).not.toHaveBeenCalled();
    gate.tick(1100);
    expect(onExpose).toHaveBeenCalledExactlyOnceWith(1, 0, 100);
    expect(gate.nextDeadline()).toBeNull();
  });

  it("never starts below 50 percent", () => {
    const onExpose = vi.fn();
    const gate = createExposureGate({ onExpose });
    gate.update(1, 0, 0.49, 0);
    gate.tick(10000);
    expect(onExpose).not.toHaveBeenCalled();
    expect(gate.nextDeadline()).toBeNull();
  });

  it("restarts after leaving the viewport, while above-threshold updates keep the start", () => {
    const onExpose = vi.fn();
    const gate = createExposureGate({ onExpose });
    gate.update(1, 2, 1, 0);
    gate.update(1, 2, 0.49, 900);
    gate.update(1, 2, 0.5, 950);
    gate.update(1, 2, 1, 1500);
    gate.tick(1949);
    expect(onExpose).not.toHaveBeenCalled();
    gate.tick(1950);
    expect(onExpose).toHaveBeenCalledExactlyOnceWith(1, 2, 950);
  });

  it("discards dwell while inactive and starts again on activation", () => {
    const onExpose = vi.fn();
    const gate = createExposureGate({ onExpose });
    gate.update(1, 0, 1, 0);
    gate.setActive(false, 900);
    gate.update(2, 1, 1, 1000);
    gate.tick(10000);
    expect(onExpose).not.toHaveBeenCalled();
    gate.setActive(true, 10000);
    gate.setActive(true, 10500);
    gate.tick(10999);
    expect(onExpose).not.toHaveBeenCalled();
    gate.tick(11000);
    expect(onExpose.mock.calls).toEqual([[1, 0, 10000], [2, 1, 10000]]);
  });

  it("exposes each article only once, including after forget and re-observation", () => {
    const onExpose = vi.fn();
    const gate = createExposureGate({ onExpose });
    gate.update(1, 0, 1, 0);
    gate.tick(1000);
    gate.forget(1);
    gate.update(1, 3, 1, 2000);
    gate.setActive(false, 2100);
    gate.setActive(true, 2200);
    gate.tick(10000);
    expect(onExpose).toHaveBeenCalledExactlyOnceWith(1, 0, 0);
  });

  it("keeps independent deadlines and forgets an unfinished article", () => {
    const onExpose = vi.fn();
    const gate = createExposureGate({ onExpose });
    gate.update(1, 0, 1, 0);
    gate.update(2, 1, 1, 500);
    gate.update(3, 2, 1, 750);
    gate.forget(3);
    expect(gate.nextDeadline()).toBe(1000);
    gate.tick(1000);
    expect(onExpose.mock.calls).toEqual([[1, 0, 0]]);
    expect(gate.nextDeadline()).toBe(1500);
    gate.tick(1500);
    expect(onExpose.mock.calls).toEqual([[1, 0, 0], [2, 1, 500]]);
  });
});
