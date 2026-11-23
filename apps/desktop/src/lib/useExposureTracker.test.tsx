import { act, useCallback, useRef, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { createReactHarness } from "../test/react";
import { FAKE_CLOCK_ORIGIN } from "../test/clock";
import { currentExposureSession, trackExposure } from "./signals";
import { useExposureTracker } from "./useExposureTracker";

vi.mock("./signals", () => ({ trackExposure: vi.fn(), currentExposureSession: vi.fn((now: number) => String(Math.floor(now / 86_400_000))) }));
const harness = createReactHarness();
afterEach(() => {
  harness.cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

function setup(initialActive = true, explore = false) {
  let entries!: IntersectionObserverCallback;
  const observer = { observe: vi.fn(), unobserve: vi.fn(), disconnect: vi.fn(), takeRecords: vi.fn(() => [] as IntersectionObserverEntry[]) };
  const construct = vi.fn(function (callback: IntersectionObserverCallback) { entries = callback; return observer; });
  vi.stubGlobal("IntersectionObserver", construct);
  let visibility = "visible";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility as DocumentVisibilityState);
  let setActive!: (value: boolean) => void;
  let setSecond!: (value: boolean) => void;
  let renders = 0;
  function Probe() {
    renders++;
    const [active, updateActive] = useState(initialActive);
    const [second, updateSecond] = useState(true);
    setActive = updateActive;
    setSecond = updateSecond;
    const root = useRef<HTMLDivElement>(null);
    const observe = useExposureTracker({ surface: "focus", root, active });
    const firstRef = useCallback((element: HTMLDivElement | null) => observe(element, 1, 0), [observe]);
    const secondRef = useCallback((element: HTMLDivElement | null) => observe(element, 2, 1, !explore, explore), [observe]);
    return <div ref={root}><div ref={firstRef} data-article="1" />{second && <div ref={secondRef} data-article="2" />}</div>;
  }
  const mounted = harness.mount(<Probe />);
  const root = mounted.host.firstElementChild;
  const entry = (id: number, ratio: number) => ({ target: mounted.host.querySelector(`[data-article="${id}"]`)!, intersectionRatio: ratio }) as IntersectionObserverEntry;
  return {
    ...mounted, observer, construct, root, entry,
    get renders() { return renders; },
    intersect(id: number, ratio: number) { act(() => entries([entry(id, ratio)], observer as unknown as IntersectionObserver)); },
    active(value: boolean) { act(() => setActive(value)); },
    second(value: boolean) { act(() => setSecond(value)); },
    visible(value: boolean) {
      visibility = value ? "visible" : "hidden";
      act(() => document.dispatchEvent(new Event("visibilitychange")));
    },
  };
}

it("uses one earliest-deadline timer, retains dwell across rendering and forgets removed rows", async () => {
  const clock = harness.useClock();
  const probe = setup();
  expect(probe.construct).toHaveBeenCalledWith(expect.any(Function), { root: probe.root, threshold: [0, 0.5, 1] });
  probe.intersect(1, 0.5);
  await clock.advance(500);
  probe.intersect(2, 1);
  expect(vi.getTimerCount()).toBe(1);
  probe.second(false);
  const renders = probe.renders;
  await clock.advance(500);
  expect(trackExposure).toHaveBeenCalledExactlyOnceWith(1, "focus", 0, FAKE_CLOCK_ORIGIN, false, false);
  expect(probe.renders).toBe(renders);
  expect(vi.getTimerCount()).toBe(1); // The same timer now waits for midnight.
  expect(probe.observer.unobserve).toHaveBeenCalledOnce();
  probe.second(true);
  probe.intersect(2, 1);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenLastCalledWith(2, "focus", 1, FAKE_CLOCK_ORIGIN + 1000, true, false);
});

it("combines active and document visibility, resets dwell, and cleans up on unmount", async () => {
  const clock = harness.useClock();
  const probe = setup();
  probe.intersect(1, 1);
  await clock.advance(900);
  probe.active(false);
  expect(vi.getTimerCount()).toBe(0);
  probe.visible(false);
  probe.active(true);
  await clock.advance(2000);
  expect(trackExposure).not.toHaveBeenCalled();
  probe.visible(true);
  await clock.advance(999);
  expect(trackExposure).not.toHaveBeenCalled();
  await clock.advance(1);
  expect(trackExposure).toHaveBeenCalledExactlyOnceWith(1, "focus", 0, FAKE_CLOCK_ORIGIN + 2900, false, false);
  probe.intersect(2, 1);
  probe.unmount();
  expect(probe.observer.disconnect).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  probe.visible(false);
  probe.visible(true);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledTimes(1);
});

it("processes queued exits before the deadline tick", async () => {
  const clock = harness.useClock();
  const probe = setup();
  probe.intersect(1, 1);
  probe.observer.takeRecords.mockReturnValueOnce([probe.entry(1, 0.49)]);
  await clock.advance(1000);
  expect(trackExposure).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(1);
});

it("reobserves after cross-day keepalive activation, stays silent while hidden and deduplicates within the day", async () => {
  const clock = harness.useClock();
  const probe = setup();
  probe.intersect(1, 1);
  await clock.advance(1000);
  probe.active(false);
  probe.visible(false);
  vi.mocked(currentExposureSession).mockClear();
  await clock.advance(7 * 86_400_000);
  probe.active(true);
  probe.intersect(1, 1);
  await clock.advance(1000);
  expect(currentExposureSession).not.toHaveBeenCalled();
  expect(trackExposure).toHaveBeenCalledTimes(1);
  probe.visible(true);
  expect(probe.observer.disconnect).toHaveBeenCalledOnce();
  expect(probe.observer.observe).toHaveBeenCalledTimes(4);
  probe.intersect(1, 0.49);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledTimes(1);
  probe.intersect(1, 0.5);
  probe.intersect(2, 1);
  await clock.advance(999);
  expect(trackExposure).toHaveBeenCalledTimes(1);
  await clock.advance(1);
  expect(vi.mocked(trackExposure).mock.calls.map(([id]) => id)).toEqual([1, 1, 2]);
  probe.active(false);
  probe.active(true);
  probe.intersect(1, 1);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledTimes(3);
});

it("rotates at UTC midnight during continuous browsing and starts a fresh one-second dwell", async () => {
  const clock = harness.useClock();
  vi.setSystemTime(FAKE_CLOCK_ORIGIN + 86_400_000 - 1500);
  const probe = setup();
  probe.intersect(1, 1);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledTimes(1);
  await clock.advance(500);
  expect(probe.observer.disconnect).toHaveBeenCalledOnce();
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledTimes(1); // Fresh intersection records are required.
  probe.intersect(1, 0.5);
  await clock.advance(999);
  expect(trackExposure).toHaveBeenCalledTimes(1);
  await clock.advance(1);
  expect(trackExposure).toHaveBeenLastCalledWith(1, "focus", 0, FAKE_CLOCK_ORIGIN + 86_401_000, false, false);
});

it("checks the day on focus after suspended timers and discards the old dwell", async () => {
  const clock = harness.useClock();
  const probe = setup();
  probe.intersect(1, 1);
  await clock.advance(900);
  vi.setSystemTime(Date.now() + 86_400_000);
  act(() => window.dispatchEvent(new Event("focus")));
  await clock.advance(1000);
  expect(trackExposure).not.toHaveBeenCalled();
  probe.intersect(1, 1);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledExactlyOnceWith(1, "focus", 0, FAKE_CLOCK_ORIGIN + 86_401_900, false, false);
});

it("reobserves on first activation when an inactive tracker was mounted the day before", async () => {
  const clock = harness.useClock();
  const probe = setup(false);
  probe.intersect(1, 1);
  await clock.advance(86_400_000);
  probe.active(true);
  expect(probe.observer.disconnect).toHaveBeenCalledOnce();
  await clock.advance(1000);
  expect(trackExposure).not.toHaveBeenCalled();
  probe.intersect(1, 1);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledTimes(1);
});

it("does nothing when IntersectionObserver is unavailable", async () => {
  const clock = harness.useClock();
  vi.stubGlobal("IntersectionObserver", undefined);
  function Probe() {
    const root = useRef<HTMLDivElement>(null);
    const observe = useExposureTracker({ surface: "focus", root, active: true });
    return <div ref={root}><div ref={element => observe(element, 1, 0)} /></div>;
  }
  harness.mount(<Probe />);
  await clock.advance(10000);
  expect(trackExposure).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it("forwards exploration through the dwell gate", async () => {
  const clock = harness.useClock();
  const probe = setup(true, true);
  probe.intersect(2, 0.5);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenCalledExactlyOnceWith(2, "focus", 1, FAKE_CLOCK_ORIGIN, false, true);
});
