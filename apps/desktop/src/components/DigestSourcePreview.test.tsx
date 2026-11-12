import { act } from "react";
import { afterEach, expect, it, vi } from "vitest";
import type { DigestSourceView } from "../bindings";
import { createReactHarness } from "../test/react";
import { DigestSourceIndex, DigestSourceMarks, DigestSourcePreviewProvider } from "./DigestSourcePreview";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.useRealTimers(); });

const source = (article_id: number, number: number | null, status = "available"): DigestSourceView => ({
  article_id, title: `文章 ${article_id}`, feed_title: `源 ${article_id}`, number,
  summary: `摘要 ${article_id}`, published_at: null, status,
});

const card = () => document.body.querySelector(".digest-source-card");
const pointer = (target: Element, type: "pointerover" | "pointerout") =>
  act(() => { target.dispatchEvent(new MouseEvent(type, { bubbles: true, relatedTarget: document.body })); });

it("keeps one card across button and card hover, and Escape closes only the card", () => {
  vi.useFakeTimers();
  const onKey = vi.fn();
  window.addEventListener("keydown", onKey);
  const { host } = harness.mount(
    <DigestSourcePreviewProvider>
      <DigestSourceMarks sources={[source(1, 1), source(2, 2)]} onOpenArticle={() => {}} />
    </DigestSourcePreviewProvider>,
  );
  const [first, second] = host.querySelectorAll(".digest-ref");
  pointer(first, "pointerover");
  expect(card()?.textContent).toContain("摘要 1");
  expect(first.getAttribute("aria-describedby")).toBe("digest-source-card");
  pointer(second, "pointerover");
  expect(document.body.querySelectorAll(".digest-source-card")).toHaveLength(1);
  expect(card()?.textContent).toContain("摘要 2");
  pointer(second, "pointerout");
  pointer(card()!, "pointerover");
  act(() => { vi.advanceTimersByTime(500); });
  expect(card()).not.toBeNull();
  pointer(card()!, "pointerout");
  act(() => { vi.advanceTimersByTime(500); });
  expect(card()).toBeNull();

  pointer(first, "pointerover");
  act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
  expect(card()).toBeNull();
  expect(onKey).not.toHaveBeenCalled();
  window.removeEventListener("keydown", onKey);
});

it("previews removed sources without opening them", () => {
  const opened = vi.fn();
  const { host } = harness.mount(
    <DigestSourcePreviewProvider>
      <DigestSourceMarks sources={[source(7, 1, "removed")]} onOpenArticle={opened} />
    </DigestSourcePreviewProvider>,
  );
  const mark = host.querySelector<HTMLButtonElement>(".digest-ref")!;
  expect(mark.disabled).toBe(false);
  expect(mark.getAttribute("aria-disabled")).toBe("true");
  act(() => { mark.focus(); });
  expect(card()?.textContent).toContain("文章已移除");
  expect(card()?.textContent).not.toContain("摘要 7");
  act(() => { mark.click(); });
  expect(opened).not.toHaveBeenCalled();
});

it("lists unlocated snapshots apart from numbered sources", () => {
  const { host } = harness.mount(
    <DigestSourcePreviewProvider>
      <DigestSourceIndex sources={[]} unlocated={[source(3, null), source(4, null)]} legacy onOpenArticle={() => {}} />
    </DigestSourcePreviewProvider>,
  );
  const summaries = [...host.querySelectorAll("summary")].map((node) => node.textContent);
  expect(summaries).toEqual(["旧稿来源（未定位）（2）"]);
  expect(host.querySelector(".digest-source-index-no")).toBeNull();
});
