import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RulesPane } from "./RulesPane";
import { api, type RulesPage } from "../lib/api";
import { deferred } from "../test/deferred";
import { rulesPageFixture } from "../test/fixtures/settings";
import { rulesKeys, rulesQueryOptions } from "./rulesQuery";

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

const PAGE: RulesPage = rulesPageFixture({
  presets: [
    {
      id: "dev-focus",
      name: "开发关注",
      description: "Rust 等",
      rule_count: 3,
      enabled_count: 0,
      installed: false,
    },
  ],
});

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: {
      ...actual.api,
      rules: vi.fn(async () => PAGE),
      createRule: vi.fn(async () => 2),
      setRuleAction: vi.fn(async () => {}),
      applyPreset: vi.fn(async () => 3),
      setSetting: vi.fn(async (id: string, value: string) => ({ id, value })),
    },
  };
});

const cleanups = new Set<() => void>();
function mount(node: ReactNode, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
  });
  const unmount = () => {
    if (!cleanups.delete(unmount)) return;
    act(() => root.unmount());
    client.clear();
    host.remove();
  };
  cleanups.add(unmount);
  return { host, root, client, unmount };
}

describe("RulesPane notifies focus after mutations", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("__TAURI_INTERNALS__", {});
    vi.mocked(api.rules).mockReset().mockResolvedValue(PAGE);
    vi.mocked(api.createRule).mockReset().mockResolvedValue(2);
    vi.mocked(api.setRuleAction).mockReset().mockResolvedValue(undefined);
    vi.mocked(api.applyPreset).mockReset().mockResolvedValue(3);
    vi.mocked(api.setSetting).mockReset().mockImplementation(async (id, value) => ({ id, value }));
  });
  afterEach(() => {
    for (const cleanup of cleanups) cleanup();
    document.body.replaceChildren();
    vi.clearAllMocks();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  async function flush() {
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(1);
    });
  }

  it("does not notify on mount or when the callback identity changes", async () => {
    const first = vi.fn();
    const { host, root, client, unmount } = mount(<RulesPane onChanged={first} />);
    await flush();
    expect(first).not.toHaveBeenCalled();
    const second = vi.fn();
    act(() => {
      root.render(<QueryClientProvider client={client}><RulesPane onChanged={second} /></QueryClientProvider>);
    });
    await flush();
    expect(first).not.toHaveBeenCalled();
    expect(second).not.toHaveBeenCalled();
    expect(host.textContent).toContain("Rust");
    unmount();
  });

  it("notifies after creating a rule", async () => {
    const onChanged = vi.fn();
    const { host, unmount } = mount(<RulesPane onChanged={onChanged} />);
    await flush();
    const input = host.querySelector('input[aria-label="匹配内容"]') as HTMLInputElement;
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    act(() => {
      setValue?.call(input, "library");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      clickText(host, "添加");
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(api.createRule).toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("switches rules off and restores each original direction", async () => {
    vi.mocked(api.rules).mockResolvedValue({ ...PAGE, rules: [
      { ...PAGE.rules[0], id: 1, enabled: false },
      { ...PAGE.rules[0], id: 2, pattern: "抽奖", polarity: "dislike", enabled: false },
      { ...PAGE.rules[0], id: 3, pattern: "已启用" },
    ] });
    const { host } = mount(<RulesPane />); await flush();
    for (const pattern of ["Rust", "抽奖", "已启用"]) {
      act(() => host.querySelector<HTMLButtonElement>(`[aria-label="启用规则 ${pattern}"]`)!.click());
      await flush(); await flush();
    }
    expect(vi.mocked(api.setRuleAction).mock.calls).toEqual([[1, 0], [2, 1], [3, 2]]);
  });

  it("builds a rule sentence with the existing validation and scope", async () => {
    vi.mocked(api.rules).mockResolvedValue({ ...PAGE, folders: [{ id: 11, label: "技术" }] });
    const { host } = mount(<RulesPane />); await flush();
    clickText(host, "添加"); await flush();
    expect(api.createRule).not.toHaveBeenCalled();
    expect(host.textContent).toContain("请填写匹配内容");
    changeInput(host.querySelector<HTMLInputElement>('[aria-label="匹配内容"]')!, "  Rust  ");
    clickText(host, "减");
    changeInput(host.querySelector<HTMLInputElement>('[aria-label="评分权重"]')!, "20");
    clickText(host, "文件夹"); clickText(host, "技术");
    clickText(host, "添加"); await flush(); await flush();
    expect(api.createRule).toHaveBeenCalledExactlyOnceWith({ polarity: "dislike", dimension: "title_kw", pattern: "Rust", weight: 20, scope_kind: "folder", scope_id: 11 });
    expect(host.querySelector<HTMLInputElement>('[aria-label="匹配内容"]')!.value).toBe("");
  });

  it.each([0, 60_000])("performs one authoritative reread after writing with cache age %i", async (age) => {
    const onChanged = vi.fn();
    const { host, client } = mount(<RulesPane onChanged={onChanged} />);
    await flush();
    act(() => {
      client.setQueryData(rulesKeys.page("all"), PAGE, { updatedAt: Date.now() - age });
      client.setQueryData(rulesKeys.page("pending"), PAGE);
    });
    const reread = deferred<RulesPage>();
    vi.mocked(api.rules).mockClear().mockReturnValueOnce(reread.promise);
    act(() => host.querySelector<HTMLButtonElement>('[aria-label="启用规则 Rust"]')!.click());
    await flush();
    expect(api.setRuleAction).toHaveBeenCalledTimes(1);
    expect(host.querySelector("fieldset")!.disabled).toBe(true);
    expect(onChanged).not.toHaveBeenCalled();
    const updated = { ...PAGE, rules: [{ ...PAGE.rules[0], enabled: false }] };
    await act(async () => { reread.resolve(updated); });
    await flush(); await flush();
    expect(api.rules).toHaveBeenCalledTimes(1);
    expect(client.getQueryData(rulesKeys.page("all"))).toEqual(updated);
    expect(client.getQueryState(rulesKeys.page("pending"))!.isInvalidated).toBe(true);
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("uses the authoritative threshold after successful save and protects drafts on refresh", async () => {
    const { host, client } = mount(<RulesPane />); await flush();
    changeInput(host.querySelector<HTMLInputElement>('input[aria-label="匹配内容"]')!, "unsaved rule");
    changeInput(thresholdInput(host), "-25");
    vi.mocked(api.rules).mockResolvedValue({ ...PAGE, hide_threshold: -30 });
    await act(async () => { await client.refetchQueries({ queryKey: rulesKeys.page("all") }); });
    await flush();
    expect(thresholdInput(host).value).toBe("-25");
    expect(host.querySelector<HTMLInputElement>('input[aria-label="匹配内容"]')!.value).toBe("unsaved rule");
    act(() => thresholdInput(host).dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
    await flush(); await flush();
    expect(thresholdInput(host).value).toBe("-30");
    expect(host.querySelector<HTMLInputElement>('input[aria-label="匹配内容"]')!.value).toBe("unsaved rule");
  });

  it("distinguishes scope loading from empty options and isolates late filter responses", async () => {
    const scopes = { ...PAGE, folders: [{ id: 11, label: "技术订阅夹" }] };
    vi.mocked(api.rules).mockResolvedValue(scopes);
    const { host } = mount(<RulesPane />); await flush();
    clickText(host, "文件夹");
    expect(host.textContent).toContain("技术订阅夹");
    const pending = deferred<RulesPage>(); vi.mocked(api.rules).mockReturnValueOnce(pending.promise);
    clickText(host, "待确认"); await flush();
    expect(host.textContent).toContain("正在读取生效范围");
    expect(host.textContent).not.toContain("暂无可选范围");
    clickText(host, "全部");
    await act(async () => { pending.resolve({ ...scopes, rules: [{ ...PAGE.rules[0], pattern: "late pending" }] }); });
    await flush();
    expect(host.querySelector(".rule-list")!.textContent).toContain("Rust");
    expect(host.querySelector(".rule-list")!.textContent).not.toContain("late pending");
    expect(host.textContent).toContain("技术订阅夹");
  });

  it("deduplicates preload and mount and rejects a prewrite response after saving", async () => {
    const old = deferred<RulesPage>(); vi.mocked(api.rules).mockReturnValueOnce(old.promise);
    const client = new QueryClient();
    const preload = client.prefetchQuery(rulesQueryOptions("all"));
    const { host } = mount(<RulesPane />, client); await flush();
    expect(api.rules).toHaveBeenCalledTimes(1);
    changeInput(host.querySelector<HTMLInputElement>('input[aria-label="匹配内容"]')!, "new rule");
    const updated = { ...PAGE, rules: [{ ...PAGE.rules[0], pattern: "new rule" }] };
    vi.mocked(api.rules).mockResolvedValue(updated);
    clickText(host, "添加"); await flush(); await flush();
    await act(async () => { old.resolve(PAGE); await preload; }); await flush();
    expect(client.getQueryData(rulesKeys.page("all"))).toEqual(updated);
    expect(api.rules).toHaveBeenCalledTimes(2);
    expect(host.querySelector(".rule-list")!.textContent).toContain("new rule");
  });
});

function thresholdInput(host: HTMLElement) {
  return host.querySelector<HTMLInputElement>('input[aria-label="隐藏阈值"]')!;
}
function changeInput(input: HTMLInputElement, value: string) {
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function clickText(host: HTMLElement, name: string) {
  const btn = [...host.querySelectorAll("button")].find((b) => b.textContent === name);
  expect(btn, name).toBeTruthy();
  act(() => {
    (btn as HTMLButtonElement).click();
  });
}
