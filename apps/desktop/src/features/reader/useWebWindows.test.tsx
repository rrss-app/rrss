import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WebViewBounds } from "../../bindings";
import { createApiStub, type ApiStub } from "../../test/api";
import { deferred } from "../../test/deferred";
import { createEventStub, type EventStub } from "../../test/events";
import { createReactHarness } from "../../test/react";
import { clickText, handleReaderReads } from "../../test/fixtures/shell";
import { networkProxyFixture } from "../../test/fixtures/settings";
import { useWebWindows, WebWindowScene } from "./useWebWindows";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null, events: null as EventStub | null }));
vi.mock("../../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../../lib/api")>("../../lib/api");
  const api = await import("../../test/api"), events = await import("../../test/events");
  return { ...actual, api: api.createApiProxy(() => box.stub), ...events.eventListenerExports(() => box.events) };
});
const url = "https://web.example.test/one", otherUrl = "https://web.example.test/two", thirdUrl = "https://web.example.test/three";

describe("embedded original page", () => {
  const harness = createReactHarness();
  const opens: { url: string; bounds: WebViewBounds }[] = [];
  const visible: boolean[] = [];
  const closedLabels: string[] = [];
  const gates: ReturnType<typeof deferred<void>>[] = [];
  beforeEach(() => {
    opens.length = 0; visible.length = 0; closedLabels.length = 0; gates.length = 0;
    box.stub = createApiStub(); box.events = createEventStub();
    handleReaderReads(box.stub);
    box.stub.handle("networkProxy", () => networkProxyFixture());
    let n = 0;
    box.stub.handle("openWebView", async (page, bounds) => {
      opens.push({ url: page, bounds });
      const gate = gates.shift();
      if (gate) await gate.promise;
      return `web-${++n}`;
    });
    box.stub.handle("setWebViewVisible", (_label, value) => { visible.push(value); });
    box.stub.handle("setWebViewBounds", () => {});
    box.stub.handle("closeWebView", label => { closedLabels.push(label); });
  });
  afterEach(() => { harness.cleanup(); box.stub!.assertNoUnexpectedCalls(); expect(box.events!.activeCount()).toBe(0); });
  function setup() {
    let controller!: ReturnType<typeof useWebWindows>;
    const calls = { show: vi.fn(), close: vi.fn(), notify: vi.fn(), openLink: vi.fn(), openExtracted: vi.fn() };
    function Probe() {
      const [scene, setScene] = useState(true);
      const [covered, setCovered] = useState(false);
      controller = useWebWindows(calls);
      return <>
        <button type="button" onClick={() => setScene(value => !value)}>切换场景</button>
        <button type="button" onClick={() => setCovered(value => !value)}>切换遮盖</button>
        {scene ? <WebWindowScene controller={controller} covered={covered} onBack={calls.close} onOpenLink={calls.openLink} onProxySettings={() => {}} /> : null}
      </>;
    }
    const { host } = harness.mount(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><Probe /></QueryClientProvider>);
    return { host, calls, get controller() { return controller; } };
  }

  it("reuses the current page, drops a late open, and leaves only for its label", async () => {
    const ui = setup();
    const launch = async (page: string) => {
      let pending!: Promise<void>;
      act(() => { pending = ui.controller.launch(page); });
      await pending;
      await act(async () => {});
    };
    await launch(url);
    const slot = ui.host.querySelector(".web-embed-slot")!;
    const rect = slot.getBoundingClientRect();
    expect(opens).toEqual([{ url, bounds: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } }]);
    expect(visible).toEqual([true]);
    expect(ui.controller.embedded).toBe(true);
    ui.controller.reportBounds({ x: 4, y: 8, width: 120, height: 80 });
    ui.controller.reportBounds({ x: 4, y: 8, width: 120, height: 80 });
    expect(box.stub!.callCount("setWebViewBounds")).toBe(1);

    clickText(ui.host, "切换遮盖");
    await act(async () => {});
    expect(ui.host.textContent).toContain("网页暂时收起");
    expect(visible).toEqual([true, false]);
    expect(ui.controller.embedded).toBe(false);
    clickText(ui.host, "切换遮盖");
    await act(async () => {});
    expect(ui.host.textContent).not.toContain("网页暂时收起");
    expect(visible).toEqual([true, false, true]);

    clickText(ui.host, "切换场景");
    await act(async () => {});
    expect(visible.at(-1)).toBe(false);
    expect(closedLabels).toEqual([]);
    clickText(ui.host, "切换场景");
    await launch(url);
    expect(opens).toHaveLength(1);
    expect(visible.at(-1)).toBe(true);

    act(() => box.events!.emit("web:navigated", { label: "web-1", url: otherUrl, title: "换页" }));
    await launch(otherUrl);
    expect(opens).toHaveLength(1);
    expect(ui.controller.current?.url).toBe(otherUrl);

    const gate = deferred<void>();
    gates.push(gate);
    let late!: Promise<void>;
    act(() => { late = ui.controller.launch(thirdUrl); });
    await harness.flush();
    expect(opens.map(item => item.url)).toEqual([url, thirdUrl]);
    act(() => ui.controller.cancel());
    await act(async () => { gate.resolve(); await late; });
    expect(closedLabels).toEqual(["web-2"]);
    expect(ui.controller.current?.label).toBe("web-1");

    act(() => box.events!.emit("web:escape", { label: "web-2" }));
    expect(ui.calls.openLink).not.toHaveBeenCalled();
    act(() => box.events!.emit("web:escape", { label: "web-1" }));
    expect(ui.calls.close).toHaveBeenCalledOnce();
    expect(ui.calls.openLink).toHaveBeenCalledWith(otherUrl, "", "reader");
    await act(async () => { ui.controller.release(); });
    expect(closedLabels).toEqual(["web-2", "web-1"]);
    expect(ui.controller.current).toBeNull();
  });
});
