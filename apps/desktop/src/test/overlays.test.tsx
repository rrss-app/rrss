import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { createApiStub, type ApiStub } from "./api";
import { createEventStub, type EventStub } from "./events";
import { readerView } from "./fixtures/reader";
import { click, clickText, handleShellReads, pressKey } from "./fixtures/shell";
import { createReactHarness } from "./react";

const box = vi.hoisted(() => ({
  stub: null as ApiStub | null,
  events: null as EventStub | null,
}));

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  const api = await import("./api");
  const events = await import("./events");
  return {
    ...actual,
    api: api.createApiProxy(() => box.stub),
    ...events.eventListenerExports(() => box.events),
  };
});

/** 设置面板替身：只暴露「弄脏 / 置忙」两个开关，验证根壳的通用离开守卫接线。 */
vi.mock("../settings/SettingsPanel", () => ({
  SettingsPanel: (props: { onEditStateChange: (state: { label: string; dirty: boolean; busy: boolean }) => void }) => (
    <div>
      <button type="button" onClick={() => props.onEditStateChange({ label: "替身设置", dirty: true, busy: false })}>弄脏</button>
      <button type="button" onClick={() => props.onEditStateChange({ label: "替身设置", dirty: false, busy: true })}>置忙</button>
    </div>
  ),
}));

describe("overlay stack priorities", () => {
  const harness = createReactHarness();
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };

  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    box.stub = createApiStub();
    box.events = createEventStub();
    handleShellReads(box.stub);
    box.stub.handle("openArticle", (id: number) => readerView(id));
    // 关闭设置后根壳预载各分类；替身面板不渲染它们，返回空载荷即可。
    for (const name of ["siteRulesPage", "rules", "memory", "translatePage", "pi", "diagnostics", "retentionPage", "backupPage"] as const) {
      box.stub.handle(name, (() => ({})) as never);
    }
  });

  afterEach(() => {
    harness.cleanup();
    vi.restoreAllMocks();
    delete win.__TAURI_INTERNALS__;
    box.stub?.assertNoUnexpectedCalls();
  });

  async function mountShell() {
    const { host } = harness.mount(<App />);
    await harness.flush();
    return host;
  }

  async function openReader(host: HTMLElement) {
    const card = [...host.querySelectorAll("article")].find((a) => a.textContent?.includes("甲文"));
    click(card);
    await harness.flush();
    expect(host.querySelector(".reader-stage")).not.toBeNull();
  }

  const settings = (host: HTMLElement) => host.querySelector(".settings-drawer");
  const reader = (host: HTMLElement) => host.querySelector(".reader-stage");
  const palette = (host: HTMLElement) => host.querySelector(".command-palette");
  const confirm = (host: HTMLElement) => host.querySelector("[role=alertdialog]");

  it("Escape peels nested layers one at a time: search, then settings, then reader", async () => {
    const host = await mountShell();
    await openReader(host);
    clickText(host, "设置");
    await harness.flush();
    expect(settings(host)).not.toBeNull();
    expect(reader(host)?.hasAttribute("inert")).toBe(true);

    pressKey("k", { ctrlKey: true });
    await harness.flush();
    expect(palette(host)).not.toBeNull();
    expect(settings(host)?.closest(".overlay")?.hasAttribute("inert")).toBe(true);

    pressKey("Escape");
    await harness.flush();
    expect(palette(host)).toBeNull();
    expect(settings(host)).not.toBeNull();
    expect(reader(host)).not.toBeNull();

    // 阅读器快捷键在设置压顶时不生效。
    pressKey("m");
    await harness.flush();
    expect(box.stub!.callCount("markRead")).toBe(0);

    pressKey("Escape");
    await harness.flush();
    expect(settings(host)).toBeNull();
    expect(reader(host)).not.toBeNull();
    expect(reader(host)?.hasAttribute("inert")).toBe(false);

    pressKey("Escape");
    await harness.flush();
    expect(reader(host)).toBeNull();
    expect(host.textContent).toContain("甲文");
  });

  it("guards a dirty settings page on Escape, browser back and mouse back, then lets the confirm discard", async () => {
    const host = await mountShell();
    clickText(host, "设置");
    await harness.flush();
    clickText(host, "弄脏");

    pressKey("Escape");
    await harness.flush();
    expect(confirm(host)?.textContent).toContain("有未保存的修改");
    expect(settings(host)).not.toBeNull();

    // 确认条压顶时 Escape 只取消确认。
    pressKey("Escape");
    await harness.flush();
    expect(confirm(host)).toBeNull();
    expect(settings(host)).not.toBeNull();

    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await harness.flush();
    expect(confirm(host)).not.toBeNull();
    expect(settings(host)).not.toBeNull();
    click(host.querySelector("[data-confirm-cancel]"));
    await harness.flush();
    expect(confirm(host)).toBeNull();

    act(() => {
      window.dispatchEvent(new MouseEvent("mouseup", { button: 3, bubbles: true, cancelable: true }));
    });
    await harness.flush();
    expect(confirm(host)).not.toBeNull();
    click(host.querySelector("[data-confirm-ok]"));
    await harness.flush();
    expect(confirm(host)).toBeNull();
    expect(settings(host)).toBeNull();
  });

  it("refuses to leave while the settings page is busy and pops other layers on browser back", async () => {
    const host = await mountShell();
    clickText(host, "设置");
    await harness.flush();
    clickText(host, "置忙");
    pressKey("Escape");
    await harness.flush();
    expect(settings(host)).not.toBeNull();
    expect(host.querySelector(".toast")?.textContent).toContain("正在保存");
    expect(confirm(host)).toBeNull();

    click(host.querySelector(".toast button"));
    await harness.flush();
    // 忙碌解除后需要真正离开；替身没有「解除」开关，这里重新挂载验证浏览器后退。
    harness.cleanup();
    const fresh = await mountShell();
    clickText(fresh, "关于");
    await harness.flush();
    expect(fresh.querySelector(".about-brand")).not.toBeNull();
    act(() => {
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await harness.flush();
    expect(fresh.querySelector(".about-brand")).toBeNull();
  });
});
