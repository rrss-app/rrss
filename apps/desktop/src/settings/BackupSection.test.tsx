import { act, type ComponentProps } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApiStub, type ApiStub } from "../test/api";
import { createReactHarness } from "../test/react";
import type { TestClock } from "../test/clock";
import type { BackupPage } from "../lib/api";
import { BackupSection } from "./BackupSection";
import type { Ask } from "./SettingsPanel";

const box = vi.hoisted(() => ({ stub: null as ApiStub | null, downloads: [] as Array<{ name: string; text: string }> }));
vi.mock("../lib/api", async () => {
  const { createApiProxy } = await import("../test/api");
  return { api: createApiProxy(() => box.stub) };
});
vi.mock("./io", () => ({ downloadText: (name: string, text: string) => { box.downloads.push({ name, text }); } }));
const harness = createReactHarness();

function page(overrides: Partial<BackupPage> = {}): BackupPage {
  return {
    last_manual: "上次备份：9 月 20 日 22:40。",
    schedule: "off",
    auto_summary: "升级前自动 · 定时关闭",
    entries: [
      { path: "/data/backups/a.zip", when: "今天 09:12", note: "升级前自动备份 · v14 · 1.21 GB", restorable: true },
      { path: "/data/backups/broken.zip", when: "9 月 1 日 08:00", note: "无法读取的文件 · 1 KB", restorable: false },
    ],
    restore_notice: null,
    rules: 64, interests: 12, memories: 37,
    ...overrides,
  };
}
const CHECK = { path: "/tmp/b.zip", file_name: "b.zip", taken: "备份于 9 月 20 日 22:40 · 版本 v14", summary: "文章 3 篇 · 订阅 1 个 · 规则 0 条 · 对话 0 个" };

let client: QueryClient;
let clock: TestClock;
function mount(props: Partial<ComponentProps<typeof BackupSection>> = {}) {
  const onAsk = vi.fn<Ask>();
  const view = harness.mount(<QueryClientProvider client={client}><BackupSection onAsk={onAsk} blocked={false} {...props} /></QueryClientProvider>);
  return { ...view, onAsk };
}
async function flush() { await harness.flush(); await clock.advance(1); await harness.flush(); }
function button(host: HTMLElement, text: string) { return [...host.querySelectorAll("button")].find((b) => b.textContent === text)!; }

describe("backup section", () => {
  beforeEach(() => {
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    clock = harness.useClock();
    client = new QueryClient();
    box.downloads = [];
    box.stub = createApiStub();
    box.stub.handle("backupPage", () => page());
  });
  afterEach(() => {
    harness.cleanup();
    client.clear();
    vi.unstubAllGlobals();
    box.stub?.assertNoUnexpectedCalls();
  });

  it("keeps current data usable when the backup is rejected or preparation fails", async () => {
    box.stub!.handle("backupInspect", () => { throw new Error("备份的数据库版本 v99 高于当前程序支持的 v15，请先升级程序"); });
    const { host, onAsk } = mount(); await flush();
    act(() => button(host, "选择备份文件…").click()); await flush();
    expect(onAsk).not.toHaveBeenCalled();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("无法使用这个备份：");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("v99");

    box.stub!.handle("backupInspect", () => CHECK);
    box.stub!.handle("backupRestore", () => { throw new Error("no space left on device"); });
    act(() => button(host, "选择备份文件…").click()); await flush();
    act(() => onAsk.mock.calls[0][2]()); await flush();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("准备恢复失败，当前数据未改动");
    expect(host.textContent).not.toContain("正在备份当前数据");
    expect(button(host, "选择备份文件…").disabled).toBe(false);
  });

  it("exports only the ticked parts and imports with a skipped list", async () => {
    const exported: unknown[] = [];
    box.stub!.handle("prefsExport", (parts) => { exported.push(parts); return "{\"kind\":\"rrss-prefs\"}"; });
    const imported: string[] = [];
    box.stub!.handle("prefsImport", (text: string) => {
      imported.push(text);
      return { summary: "规则 新增 1 条", skipped: ["规则 不喜欢标题「广告」：限定的 https://a/rss 在本机不存在"] };
    });
    const changed = vi.fn();
    const { host } = mount({ onChanged: changed }); await flush();
    const memories = [...host.querySelectorAll<HTMLInputElement>(".backup-checks input")][2];
    act(() => memories.click());
    act(() => button(host, "导出…").click()); await flush();
    expect(exported).toEqual([{ rules: true, interests: true, memories: false }]);
    expect(box.downloads[0].name).toMatch(/^rrss-prefs-\d{4}-\d{2}-\d{2}\.json$/);

    const input = host.querySelector<HTMLInputElement>('input[type="file"]')!;
    // jsdom 的 File.text() 走真实定时器；这里只需要文本内容。
    const file = { name: "prefs.json", text: async () => "{\"x\":1}" } as unknown as File;
    Object.defineProperty(input, "files", { value: [file], configurable: true });
    act(() => { input.dispatchEvent(new Event("change", { bubbles: true })); }); await flush();
    expect(imported).toEqual(["{\"x\":1}"]);
    expect(host.querySelector(".backup-result")?.textContent).toContain("导入完成：规则 新增 1 条");
    expect(host.querySelectorAll(".backup-skipped li")).toHaveLength(1);
    expect(changed).toHaveBeenCalledTimes(1);
  });

});
