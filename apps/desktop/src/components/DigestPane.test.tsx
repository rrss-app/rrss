import { act, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DigestPane } from "./DigestPane";
import { api } from "../lib/api";
import { PREVIEW_DIGESTS } from "../lib/preview";
import { createReactHarness } from "../test/react";
import { click, clickText } from "../test/fixtures/shell";

const harness = createReactHarness();
beforeEach(() => {
  vi.spyOn(api, "digests").mockImplementation(async (period) => PREVIEW_DIGESTS.filter((digest) => digest.period === period));
});
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });
it("renders block markup rather than raw Markdown, with numbered sources opening articles", async () => {
  const onOpenArticle = vi.fn();
  const { host } = harness.mount(<DigestPane onOpenArticle={onOpenArticle} />);
  await harness.flush();
  expect(host.querySelector(".overlay")).toBeNull();
  expect(host.querySelector(".card")).toBeNull();
  expect(host.querySelector(".digest")).not.toBeNull();
  expect(host.querySelector("h3")?.textContent).toBe("今日三条主线");
  expect(host.querySelector(".digest-doc strong")?.textContent).toBe("加粗");
  expect(host.textContent).toContain("列表项");
  expect(host.textContent).not.toContain("###");
  expect(host.textContent).not.toContain("**");
  const marks = host.querySelectorAll(".digest-ref");
  expect([...marks].map((mark) => mark.textContent)).toEqual(["1", "2"]);
  const item = host.querySelector('[data-block-id="2"]')!.cloneNode(true) as HTMLElement;
  item.querySelectorAll("[data-selection-ignore]").forEach((node) => node.remove());
  expect(item.textContent).toBe("列表项");
  click(marks[0]); click(marks[1]);
  expect(onOpenArticle.mock.calls).toEqual([[1], [2]]);
  expect(host.querySelector(".digest-source-index summary")?.textContent).toBe("本期来源（2）");
  const rows = [...host.querySelectorAll(".digest-source-index-link")];
  expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual([
    "来源 1：《我用旧平板做了一块桌面信息屏》，闲聊广场",
    "来源 2：《编译器 1.97.1 点发行说明》，Compiler Notes",
  ]);
});
it("switches to weekly without leaving stale daily content", async () => {
  const { host } = harness.mount(<DigestPane onOpenArticle={() => {}} />);
  await harness.flush();
  clickText(host, "周报");
  await harness.flush();
  expect(host.querySelector("h3")?.textContent).toBe("本周三条主线");
  expect(host.textContent).not.toContain("今日三条主线");
  expect(host.querySelector(".overlay")).toBeNull();
});
it("walks daily periods with previous and next", async () => {
  const { host } = harness.mount(<DigestPane onOpenArticle={() => {}} />);
  await harness.flush();
  const next = host.querySelector<HTMLButtonElement>("button[aria-label='下期']")!;
  expect(next.disabled).toBe(true);
  click(host.querySelector("button[aria-label='上期']"));
  expect(host.querySelector("h3")?.textContent).toBe("昨日主线");
  expect(host.textContent).toContain("8月26日");
  click(next);
  expect(host.querySelector("h3")?.textContent).toBe("今日三条主线");
  expect(host.textContent).toContain("8月27日");
});

describe("kept alive while hidden", () => {
  const win = window as Window & { __TAURI_INTERNALS__?: unknown };
  const daily = PREVIEW_DIGESTS.filter((digest) => digest.period === "daily");
  const summary = ({ id, period, period_label, period_start, date_label }: (typeof daily)[number]) => ({ id, period, period_label, period_start, date_label });
  let toggle: () => void = () => {};
  function Host({ onCite }: { onCite?: () => void }) {
    const [visible, setVisible] = useState(true);
    toggle = () => setVisible((value) => !value);
    return <DigestPane visible={visible} onOpenArticle={() => {}} onCiteSelection={onCite} />;
  }
  const switchAwayAndBack = async () => {
    act(toggle);
    act(toggle);
    await harness.flush();
  };
  beforeEach(() => {
    win.__TAURI_INTERNALS__ = {};
    vi.mocked(api.digests).mockImplementation(async (period) => daily.filter((digest) => digest.period === period).map(summary));
    vi.spyOn(api, "digest").mockImplementation(async (id) => PREVIEW_DIGESTS.find((digest) => digest.id === id)!);
  });
  afterEach(() => { delete win.__TAURI_INTERNALS__; });

  it("silently rereads the period list on show and keeps the same issue without refetching it", async () => {
    const { host } = harness.mount(<Host />);
    await harness.flush();
    click(host.querySelector("button[aria-label='上期']"));
    await harness.flush();
    expect(host.querySelector("h3")?.textContent).toBe("昨日主线");
    const bodyReads = vi.mocked(api.digest).mock.calls.length;
    // 后台生成的新一期排到最前，原下标会指向别的期，选中期须按 id 找回。
    const fresh = { ...summary(daily[0]), id: 999, date_label: "8月28日" };
    vi.mocked(api.digests).mockResolvedValueOnce([fresh, ...daily.map(summary)]);
    await switchAwayAndBack();
    expect(api.digests).toHaveBeenCalledTimes(2);
    expect(api.digest).toHaveBeenCalledTimes(bodyReads);
    expect(host.querySelector("h3")?.textContent).toBe("昨日主线");
    expect(host.querySelector(".digest-date")?.textContent).toBe(daily[1].date_label);
    expect(host.querySelector("[role='status']")).toBeNull();
    click(host.querySelector("button[aria-label='下期']"));
    click(host.querySelector("button[aria-label='下期']"));
    await harness.flush();
    expect(api.digest).toHaveBeenLastCalledWith(999);
  });

  it("keeps the list and issue when the silent reread fails, with an explicit retry", async () => {
    const { host } = harness.mount(<Host />);
    await harness.flush();
    vi.mocked(api.digests).mockRejectedValueOnce(new Error("offline"));
    await switchAwayAndBack();
    expect(host.querySelector("h3")?.textContent).toBe("今日三条主线");
    expect(host.querySelector("[role='alert']")?.textContent).toContain("期列表刷新失败：offline");
    clickText(host, "重试");
    await harness.flush();
    expect(host.querySelector("[role='alert']")).toBeNull();
    expect(host.querySelector("h3")?.textContent).toBe("今日三条主线");
    expect(api.digest).toHaveBeenCalledTimes(1);
  });

  it("closes the source card and selection menu when hidden, releasing Escape", async () => {
    const { host } = harness.mount(<Host onCite={() => {}} />);
    await harness.flush();
    const escape = vi.fn();
    window.addEventListener("keydown", escape);
    act(() => { host.querySelector(".digest-ref")!.dispatchEvent(new MouseEvent("pointerover", { bubbles: true, relatedTarget: document.body })); });
    expect(document.body.querySelector(".digest-source-card")).not.toBeNull();
    act(toggle);
    expect(document.body.querySelector(".digest-source-card")).toBeNull();
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })); });
    expect(escape).toHaveBeenCalledTimes(1);

    act(toggle);
    const block = host.querySelector('.digest-doc [data-block-id="2"]')!;
    const range = document.createRange();
    range.selectNodeContents(block);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    act(() => { block.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 40, clientY: 100 })); });
    expect(document.body.querySelector(".sel-menu")).not.toBeNull();
    act(toggle);
    expect(document.body.querySelector(".sel-menu")).toBeNull();
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })); });
    expect(escape).toHaveBeenCalledTimes(2);
    window.removeEventListener("keydown", escape);
    window.getSelection()?.removeAllRanges();
  });

  it("keeps the digest selection menu beside the pointer without taking focus or closing on scroll", async () => {
    const { host } = harness.mount(<Host onCite={() => {}} />);
    await harness.flush();
    const block = host.querySelector('.digest-doc [data-block-id="2"]')!;
    const range = document.createRange();
    range.selectNodeContents(block);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    const before = document.activeElement;
    const x = window.innerWidth - 10;
    const y = window.innerHeight - 10;
    act(() => { block.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: x, clientY: y })); });
    const menu = document.body.querySelector<HTMLElement>(".sel-menu");
    expect(menu).not.toBeNull();
    expect(menu!.classList.contains("reader-selection-menu")).toBe(false);
    expect(menu!.querySelector(".icon")).toBeNull();
    expect(document.activeElement).toBe(before);
    expect(menu!.style.left).toBe(`${Math.max(8, Math.min(x, window.innerWidth - 168))}px`);
    expect(menu!.style.top).toBe(`${Math.max(8, Math.min(y + 12, window.innerHeight - 44))}px`);
    const scroller = host.querySelector(".scroll") ?? host;
    act(() => {
      window.dispatchEvent(new Event("scroll"));
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    expect(document.body.querySelector(".sel-menu")).toBe(menu);
    window.getSelection()?.removeAllRanges();
  });
});
