import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { installFakeClock, type TestClock } from "./clock";

export type Mounted = {
  host: HTMLElement;
  unmount: () => void;
};

export type ReactHarness = {
  mount: (node: ReactNode) => Mounted;
  flush: () => Promise<void>;
  cleanup: () => void;
  useClock: () => TestClock;
  readonly mountedCount: number;
};

type RootEntry = { host: HTMLElement; root: Root; alive: boolean };

function ensureActEnvironment() {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
}

/** 有界 microtask 刷新，覆盖连续 await 的写后重读；不推进计时器。 */
const FLUSH_TICKS = 8;

export async function flushUpdates() {
  ensureActEnvironment();
  await act(async () => {
    for (let i = 0; i < FLUSH_TICKS; i++) await Promise.resolve();
  });
}

function throwCleanupErrors(errors: unknown[]) {
  if (errors.length === 0) return;
  if (errors.length === 1) throw errors[0];
  throw new AggregateError(errors, "react harness cleanup failed");
}

export function createReactHarness(): ReactHarness {
  ensureActEnvironment();
  const roots: RootEntry[] = [];
  const clocks: TestClock[] = [];

  function unmountEntry(entry: RootEntry) {
    if (!entry.alive) return;
    entry.alive = false;
    try {
      act(() => {
        entry.root.unmount();
      });
    } finally {
      entry.host.remove();
    }
  }

  return {
    get mountedCount() {
      return roots.filter((entry) => entry.alive).length;
    },
    mount(node) {
      const host = document.createElement("div");
      document.body.append(host);
      const root = createRoot(host);
      const entry: RootEntry = { host, root, alive: true };
      roots.push(entry);
      act(() => {
        root.render(node);
      });
      return {
        host,
        unmount() {
          unmountEntry(entry);
        },
      };
    },
    flush: flushUpdates,
    useClock() {
      const clock = installFakeClock();
      clocks.push(clock);
      return clock;
    },
    cleanup() {
      const errors: unknown[] = [];
      for (const entry of roots) {
        try {
          unmountEntry(entry);
        } catch (err) {
          errors.push(err);
        }
      }
      roots.length = 0;
      for (const clock of clocks) {
        try {
          clock.restore();
        } catch (err) {
          errors.push(err);
        }
      }
      clocks.length = 0;
      throwCleanupErrors(errors);
    },
  };
}
