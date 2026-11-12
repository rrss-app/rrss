import { act, useCallback, useRef, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { createReactHarness } from "../test/react";
import { FAKE_CLOCK_ORIGIN } from "../test/clock";
import { trackExposure } from "./signals";
import { useExposureTracker } from "./useExposureTracker";

vi.mock("./signals", () => ({ trackExposure: vi.fn() }));
const harness = createReactHarness();
afterEach(() => {
  harness.cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

function setup() {
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
    const [active, updateActive] = useState(true);
    const [second, updateSecond] = useState(true);
    setActive = updateActive;
    setSecond = updateSecond;
    const root = useRef<HTMLDivElement>(null);
    const observe = useExposureTracker({ surface: "focus", root, active });
    const firstRef = useCallback((element: HTMLDivElement | null) => observe(element, 1, 0), [observe]);
    const secondRef = useCallback((element: HTMLDivElement | null) => observe(element, 2, 1, true), [observe]);
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
  expect(trackExposure).toHaveBeenCalledExactlyOnceWith(1, "focus", 0, FAKE_CLOCK_ORIGIN, false);
  expect(probe.renders).toBe(renders);
  expect(vi.getTimerCount()).toBe(0);
  expect(probe.observer.unobserve).toHaveBeenCalledOnce();
  probe.second(true);
  probe.intersect(2, 1);
  await clock.advance(1000);
  expect(trackExposure).toHaveBeenLastCalledWith(2, "focus", 1, FAKE_CLOCK_ORIGIN + 1000, true);
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
  expect(trackExposure).toHaveBeenCalledExactlyOnceWith(1, "focus", 0, FAKE_CLOCK_ORIGIN + 2900, false);
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
  expect(vi.getTimerCount()).toBe(0);
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
