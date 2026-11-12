import { useEffect, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createEventStub } from "./events";
import { createReactHarness } from "./react";

describe("createReactHarness", () => {
  const harness = createReactHarness();
  afterEach(() => harness.cleanup());

  it("flush runs effects and explicit unmount runs listener cleanup", async () => {
    const events = createEventStub();
    function Probe() {
      useEffect(() => {
        let off: (() => void) | undefined;
        void events.onChatStart(() => {}).then((unlisten) => {
          off = unlisten;
        });
        return () => {
          off?.();
        };
      }, []);
      return <span>probe</span>;
    }
    const { host, unmount } = harness.mount(<Probe />);
    await harness.flush();
    expect(host.textContent).toBe("probe");
    expect(events.activeCount("chat:start")).toBe(1);
    unmount();
    expect(harness.mountedCount).toBe(0);
    expect(events.activeCount()).toBe(0);
    expect(events.unsubscribeCount).toBe(1);
  });

  it("cleanup unmounts so effects unsubscribe instead of erasing the event stub", async () => {
    const events = createEventStub();
    function Probe() {
      useEffect(() => {
        let off: (() => void) | undefined;
        void events.onMediaReady(() => {}).then((unlisten) => {
          off = unlisten;
        });
        return () => {
          off?.();
        };
      }, []);
      return null;
    }
    harness.mount(<Probe />);
    await harness.flush();
    expect(events.activeCount()).toBe(1);
    harness.cleanup();
    expect(events.activeCount()).toBe(0);
    expect(events.subscribeCount).toBe(1);
    expect(events.unsubscribeCount).toBe(1);
  });

  it("unmounts remaining roots and restores clocks without wiping other stubs", () => {
    const events = createEventStub();
    harness.useClock();
    expect(vi.isFakeTimers()).toBe(true);
    const { host } = harness.mount(<div>mounted</div>);
    expect(harness.mountedCount).toBe(1);
    expect(host.isConnected).toBe(true);
    expect(events.activeCount()).toBe(0);
    harness.cleanup();
    expect(harness.mountedCount).toBe(0);
    expect(host.isConnected).toBe(false);
    expect(vi.isFakeTimers()).toBe(false);
    expect(events.subscribeCount).toBe(0);
  });

  it("still unmounts after a render callback throws", () => {
    function Boom(): ReactNode {
      throw new Error("render failed");
    }
    expect(() => harness.mount(<Boom />)).toThrow("render failed");
    expect(harness.mountedCount).toBe(1);
    harness.cleanup();
    expect(harness.mountedCount).toBe(0);
  });

  it("cleanup finishes remaining roots and clocks if an effect cleanup throws", () => {
    harness.useClock();
    function BoomOnUnmount() {
      useEffect(() => () => {
        throw new Error("effect cleanup failed");
      }, []);
      return <span>boom</span>;
    }
    function Ok() {
      return <span>ok</span>;
    }
    const first = harness.mount(<BoomOnUnmount />);
    const second = harness.mount(<Ok />);
    expect(harness.mountedCount).toBe(2);
    expect(vi.isFakeTimers()).toBe(true);
    expect(() => harness.cleanup()).toThrow(/effect cleanup failed/);
    expect(harness.mountedCount).toBe(0);
    expect(first.host.isConnected).toBe(false);
    expect(second.host.isConnected).toBe(false);
    expect(vi.isFakeTimers()).toBe(false);
  });
});
