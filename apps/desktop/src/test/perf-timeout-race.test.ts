import { afterEach, describe, expect, it, vi } from "vitest";
import { raceWithTimeout } from "../../perf/timeout-race";
import { installFakeClock, type TestClock } from "./clock";

describe("raceWithTimeout", () => {
  let clock: TestClock | undefined;
  const rejections: unknown[] = [];
  function onUnhandled(reason: unknown) {
    rejections.push(reason);
  }

  afterEach(() => {
    process.off("unhandledRejection", onUnhandled);
    clock?.restore();
    clock = undefined;
    rejections.length = 0;
  });

  it("clears the timeout when work wins so later ticks do not reject", async () => {
    clock = installFakeClock();
    process.on("unhandledRejection", onUnhandled);
    const result = await raceWithTimeout(Promise.resolve("decoded"), 10_000, "img decode timeout");
    expect(result).toBe("decoded");
    expect(vi.getTimerCount()).toBe(0);
    await clock.advance(10_000);
    expect(rejections).toEqual([]);
  });

  it("clears 64 successful races before the shared budget elapses", async () => {
    clock = installFakeClock();
    process.on("unhandledRejection", onUnhandled);
    for (let i = 0; i < 64; i++) {
      await raceWithTimeout(Promise.resolve(i), 10_000, "img decode timeout");
    }
    expect(vi.getTimerCount()).toBe(0);
    await clock.advance(10_000);
    expect(rejections).toEqual([]);
  });

  it("rejects when the timeout fires first", async () => {
    clock = installFakeClock();
    const pending = new Promise<never>(() => {});
    const raced = raceWithTimeout(pending, 50, "img decode timeout");
    const expectReject = expect(raced).rejects.toThrow("img decode timeout");
    await clock.advance(50);
    await expectReject;
    expect(vi.getTimerCount()).toBe(0);
  });
});
