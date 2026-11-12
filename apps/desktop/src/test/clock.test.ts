import { afterEach, describe, expect, it, vi } from "vitest";
import { FAKE_CLOCK_ORIGIN, installFakeClock, type TestClock } from "./clock";

describe("installFakeClock", () => {
  let clock: TestClock | undefined;
  afterEach(() => {
    clock?.restore();
    clock = undefined;
  });

  it("starts Date.now at a fixed origin and only fires due timers at 5ms", async () => {
    clock = installFakeClock();
    expect(Date.now()).toBe(FAKE_CLOCK_ORIGIN);
    const early = vi.fn();
    const due = vi.fn();
    setTimeout(early, 5);
    setTimeout(due, 10);
    await clock.advance(5);
    expect(Date.now()).toBe(FAKE_CLOCK_ORIGIN + 5);
    expect(early).toHaveBeenCalledTimes(1);
    expect(due).not.toHaveBeenCalled();
    await clock.advance(5);
    expect(due).toHaveBeenCalledTimes(1);
    clock.restore();
    expect(vi.isFakeTimers()).toBe(false);
  });

  it("accepts an explicit now and rejects a second install until restore", async () => {
    clock = installFakeClock({ now: 1_000 });
    expect(Date.now()).toBe(1_000);
    expect(() => installFakeClock()).toThrow(/already installed/);
    clock.restore();
    clock.restore();
    expect(vi.isFakeTimers()).toBe(false);
    await expect(clock.advance(1)).rejects.toThrow(/already restored/);
    const again = installFakeClock();
    clock = again;
    expect(Date.now()).toBe(FAKE_CLOCK_ORIGIN);
  });

  it("does not expose unbounded timer draining", () => {
    clock = installFakeClock();
    expect(clock).not.toHaveProperty("runAllTimers");
  });

  it("refuses to replace an externally installed fake clock", () => {
    vi.useFakeTimers();
    try {
      expect(() => installFakeClock()).toThrow(/already installed/);
      expect(vi.isFakeTimers()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});
