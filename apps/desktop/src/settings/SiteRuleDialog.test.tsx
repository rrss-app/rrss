import { act } from "react";
import { QueryClient, QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { deferred } from "../test/deferred";
import { createReactHarness } from "../test/react";
import { SiteRuleDialog } from "./SiteRuleDialog";
import type { Ask } from "./SettingsPanel";
import type { SiteRuleTestView } from "../bindings";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null }));
vi.mock("../lib/api", async () => { const { createApiProxy } = await import("../test/api"); return { api: createApiProxy(() => box.stub) }; });
const harness = createReactHarness();
let client: QueryClient;
const close = vi.fn();
const ask = vi.fn<Ask>();
beforeEach(() => { client = new QueryClient(); notifyManager.setScheduler(queueMicrotask); box.stub = createApiStub(); close.mockReset(); ask.mockReset(); });
afterEach(() => { harness.cleanup(); client.clear(); box.stub!.assertNoUnexpectedCalls(); });
const mount = () => harness.mount(<QueryClientProvider client={client}><SiteRuleDialog seed={{ domain: "example.com", url: "https://example.com/story" }} onAsk={ask} onClose={close} /></QueryClientProvider>);
const button = (host: HTMLElement, text: string) => [...host.querySelectorAll("button")].find(b => b.textContent === text)!;
const click = (el: HTMLElement) => act(() => el.click());
function fill(host: HTMLElement, label: string, value: string) {
  const id = [...host.querySelectorAll("label")].find(l => l.firstElementChild?.textContent === label)!.htmlFor;
  const el = host.querySelector<HTMLInputElement>(`[id="${id}"]`)!;
  act(() => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, value); el.dispatchEvent(new Event("input", { bubbles: true })); });
}
const report = (title: string): SiteRuleTestView => ({ layer: "UserRule", title, author: null, date: null, text_chars: 800, image_count: 0, paragraphs: [title], stripped_elements: 0, elapsed_ms: 20 });

it("保留未保存草稿，确认后才离开", async () => {
  const { host } = mount();
  fill(host, "正文", "article");
  click(button(host, "取消"));
  expect(close).not.toHaveBeenCalled();
  expect(ask).toHaveBeenCalledOnce();
  expect(host.querySelectorAll("input")[1].value).toBe("article");
  act(() => ask.mock.calls[0][3]!.secondary!.onClick());
  expect(close).toHaveBeenCalledOnce();
});
it("重复保存与另一个弹窗共享写入锁，卸载后不处理迟到保存", async () => {
  const pending = box.stub!.deferNext("saveSiteRule");
  const first = mount();
  click(button(first.host, "保存")); click(button(first.host, "保存"));
  const second = mount();
  click(button(second.host, "保存"));
  await harness.flush();
  expect(box.stub!.callCount("saveSiteRule")).toBe(1);
  expect(button(second.host, "取消").disabled).toBe(true);
  first.unmount();
  pending.resolve(); await harness.flush();
  expect(close).not.toHaveBeenCalled();
});
it("可再次测试，以最后一次为准，使用未保存草稿并丢弃迟到失败", async () => {
  const old = deferred<SiteRuleTestView>();
  const latest = deferred<SiteRuleTestView>();
  const requests = [old, latest];
  const bodies: Array<string | null> = [];
  box.stub!.handle("testSiteRule", (_url, rule) => { bodies.push(rule.body); return requests.shift()!.promise; });
  const { host } = mount();
  fill(host, "正文", "article.unsaved");
  click(button(host, "测试")); click(button(host, "测试"));
  await harness.flush();
  expect(box.stub!.callCount("testSiteRule")).toBe(2);
  expect(bodies).toEqual(["article.unsaved", "article.unsaved"]);
  latest.resolve(report("latest")); await harness.flush();
  old.reject(new Error("old failure")); await harness.flush();
  expect(host.querySelector('[role="status"]')?.textContent).toContain("latest");
  expect(host.querySelector('[role="alert"]')).toBeNull();
  const late = box.stub!.deferNext("testSiteRule");
  click(button(host, "测试"));
  fill(host, "正文", "article.changed");
  late.resolve(report("stale draft")); await harness.flush();
  expect(host.querySelector('[role="status"]')).toBeNull();
});
