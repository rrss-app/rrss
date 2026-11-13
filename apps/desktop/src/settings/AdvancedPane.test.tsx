import { act } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiStub } from "../test/api";
import { createPiFixture, type PiFixture } from "../test/fixtures/pi";
import { generatedConfigFixture, networkProxyFixture } from "../test/fixtures/settings";
import { createReactHarness } from "../test/react";
import { AdvancedPane } from "./AdvancedPane";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});

const harness = createReactHarness();
let client: QueryClient;
let fixture: PiFixture;

function button(root: ParentNode, name: string) {
  const buttons = [...root.querySelectorAll("button")];
  // 先找完全一致的名字；菜单项文字带勾选图标等装饰时再退回包含匹配。
  const found = buttons.find((b) => b.textContent === name || b.getAttribute("aria-label") === name)
    ?? buttons.find((b) => b.textContent?.includes(name));
  expect(found, name).toBeTruthy();
  return found!;
}
function type(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  act(() => {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
const click = (el: HTMLElement) => act(() => el.click());
const field = <T extends Element>(host: HTMLElement, label: string) => host.querySelector<T>(`[aria-label="${label}"]`)!;

describe("AdvancedPane", () => {
  beforeEach(() => {
    client = new QueryClient();
    notifyManager.setScheduler(queueMicrotask);
    fixture = createPiFixture();
    const stub = fixture.stub;
    stub.handle("piGeneratedConfig", () => generatedConfigFixture({ ignored: ["settings.cacheWarming"] }));
    box.stub = stub;
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  });
  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    harness.cleanup();
    box.stub = null;
    client.clear();
  });

  it("keeps an invalid JSON override as a draft without saving, and clears a saved one by emptying it", async () => {
    const onEdit = vi.fn();
    const mounted = harness.mount(<QueryClientProvider client={client}><AdvancedPane onAsk={vi.fn()} onEditStateChange={onEdit} /></QueryClientProvider>);
    await harness.flush();
    const host = mounted.host;
    const box = field<HTMLTextAreaElement>(host, "Pi 设置覆盖");
    type(box, '{ "compaction": ');
    click(button(host, "保存"));
    await harness.flush();
    expect(fixture.snapshot().config.settings_override).toBe("{}");
    expect(box.value).toBe('{ "compaction": ');
    expect(host.textContent).toContain("不是有效的 JSON 对象");
    expect(onEdit).toHaveBeenLastCalledWith(expect.objectContaining({ dirty: true }));

    type(box, '{ "compaction": { "reserveTokens": 16000 } }');
    click(button(host, "保存"));
    await harness.flush();
    expect(JSON.parse(fixture.snapshot().config.settings_override)).toEqual({ compaction: { reserveTokens: 16000 } });
    // 清空即回到默认，不再有单独的「恢复默认」按钮。
    type(box, "");
    click(button(host, "保存"));
    await harness.flush();
    expect(fixture.snapshot().config.settings_override).toBe("{}");
    expect(box.value).toBe("");
  });

  it("tests every use against the unsaved draft without saving it", async () => {
    const tested: unknown[] = [];
    fixture.stub.handle("testNetworkProxy", (args) => {
      tested.push(args);
      return { ok: true, latency_ms: 42, status: 200, message: "ok" };
    });
    const mounted = harness.mount(<QueryClientProvider client={client}><AdvancedPane onAsk={vi.fn()} /></QueryClientProvider>);
    await harness.flush();
    const host = mounted.host;
    click(button(host, "全局代理模式"));
    click(button(document, "自定义代理"));
    type(field<HTMLInputElement>(host, "全局代理地址"), "http://127.0.0.1:7890");
    click(button(host, "测试连接"));
    await harness.flush();
    expect(tested.map((args) => (args as { scope: string }).scope)).toEqual(["content", "api", "pi"]);
    expect(tested[0]).toMatchObject({ draft: { global: { mode: "custom", url: "http://127.0.0.1:7890" }, content: { mode: "inherit" } } });
    expect([...host.querySelectorAll("[data-proxy-check]")].map((row) => row.getAttribute("data-state"))).toEqual(["ok", "ok", "ok"]);
    expect(fixture.stub.callCount("saveNetworkProxy")).toBe(0);
    // 草稿再变，旧结果不再对应当前线路。
    type(field<HTMLInputElement>(host, "全局代理地址"), "http://127.0.0.1:7891");
    expect([...host.querySelectorAll("[data-proxy-check][data-state]")]).toHaveLength(0);
  });

  it("marks saved authentication for clearing when the draft proxy endpoint changes", async () => {
    fixture.stub.handle("networkProxy", () =>
      networkProxyFixture({
        global: {
          mode: "custom",
          url: "http://proxy.example/",
          no_proxy: "",
          has_auth: true,
        },
      }),
    );
    const mounted = harness.mount(
      <QueryClientProvider client={client}>
        <AdvancedPane onAsk={vi.fn()} />
      </QueryClientProvider>,
    );
    await harness.flush();
    const group = mounted.host.querySelector('[data-proxy-scope="global"]')!;
    const input = group.querySelector<HTMLInputElement>(
      '[data-proxy-field="url"]',
    )!;
    const state = () =>
      group.querySelector("[data-auth-state]")!.getAttribute("data-auth-state");
    expect(state()).toBe("saved");
    for (const [address, expected] of [
      ["http://other.example", "clear"],
      ["HTTP://PROXY.EXAMPLE:80", "saved"],
      ["http://proxy.example:8080/", "clear"],
      ["https://proxy.example/", "clear"],
    ]) {
      type(input, address);
      expect(state()).toBe(expected);
      expect(
        !!group.querySelector('[data-auth-clear-reason="endpoint-changed"]'),
      ).toBe(expected === "clear");
    }
    type(input, "http://new-user:fake-pass@other.example:8080");
    expect(state()).toBe("replace");
    expect(group.querySelector("[data-auth-clear-reason]")).toBeNull();
  });

  it("moves pasted proxy credentials out of the address and keeps a client-side rejection as a draft", async () => {
    const mounted = harness.mount(<QueryClientProvider client={client}><AdvancedPane onAsk={vi.fn()} /></QueryClientProvider>);
    await harness.flush();
    const host = mounted.host;
    click(button(host, "全局代理模式"));
    click(button(document, "自定义代理"));
    const input = field<HTMLInputElement>(host, "全局代理地址");
    type(input, "127.0.0.1:7890");
    click(button(host, "保存"));
    await harness.flush();
    expect(fixture.stub.callCount("saveNetworkProxy")).toBe(0);
    expect(input.value).toBe("127.0.0.1:7890");

    type(input, "socks5h://me:p%40ss@127.0.0.1:1080");
    expect(input.value).toBe("socks5h://127.0.0.1:1080");
    expect(field<HTMLInputElement>(host, "全局代理用户名").value).toBe("me");
    expect(field<HTMLInputElement>(host, "全局代理密码").value).toBe("p@ss");
    click(button(host, "保存"));
    await harness.flush();
    expect(fixture.snapshot().networkProxy.global).toMatchObject({ url: "socks5h://127.0.0.1:1080", has_auth: true });
    // 保存后认证不回显，两栏收起。
    expect(field<HTMLInputElement>(host, "全局代理密码")).toBeNull();
  });
});
