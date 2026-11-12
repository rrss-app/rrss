import { act } from "react";
import { vi } from "vitest";

/** Vitest fake timers 的窄封装：只按毫秒推进，恢复真实时钟。不提供无界 runAllTimers。 */

export const FAKE_CLOCK_ORIGIN = Date.UTC(2024, 0, 1);

export type FakeClockOptions = {
  now?: number;
};

export type TestClock = {
  advance: (ms: number) => Promise<void>;
  restore: () => void;
};

let activeClock: TestClock | null = null;

export function installFakeClock(options: FakeClockOptions = {}): TestClock {
  if (activeClock || vi.isFakeTimers()) {
    throw new Error("fake clock already installed; Vitest timers are process-global");
  }
  vi.useFakeTimers({ now: options.now ?? FAKE_CLOCK_ORIGIN });
  let restored = false;
  const clock: TestClock = {
    async advance(ms: number) {
      if (restored) throw new Error("clock already restored");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(ms);
      });
    },
    restore() {
      if (restored) return;
      restored = true;
      if (activeClock === clock) activeClock = null;
      vi.useRealTimers();
    },
  };
  activeClock = clock;
  return clock;
}
