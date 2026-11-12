import { act, createRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../lib/api";
import { focusRow } from "../../test/fixtures/reader";
import { trainingFeaturesFixture } from "../../test/fixtures/settings";
import { createReactHarness } from "../../test/react";
import { deferred } from "../../test/deferred";
import { createArticleWriteOrder } from "../../lib/articleWriteOrder";
import { FocusTraining, type FocusTrainingHandle } from "./FocusTraining";
import type { FocusSession } from "./focusSession";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });

function setup() {
  const reload = vi.fn(async () => {});
  const session = { reload } as unknown as FocusSession;
  vi.spyOn(api, "rate").mockResolvedValue(undefined);
  vi.spyOn(api, "training").mockResolvedValue(trainingFeaturesFixture());
  const confirm = vi.spyOn(api, "confirmFeature").mockResolvedValue(0);
  const onOpen = vi.fn(), onClose = vi.fn(), notify = vi.fn();
  const ref = createRef<FocusTrainingHandle>();
  const { host } = harness.mount(<FocusTraining ref={ref} session={session} ratings={createArticleWriteOrder()} open style={{}} onOpen={onOpen} onClose={onClose} notify={notify} />);
  return { reload, confirm, onOpen, onClose, notify, ref, host };
}

it("serializes confirmation and refreshes only after a successful save, retaining failed suggestions", async () => {
  const { ref, host, reload, confirm, onClose, notify } = setup();
  const pending = deferred<number>();
  confirm.mockReturnValueOnce(pending.promise);
  await act(async () => { await ref.current!.rate(focusRow(1, "甲文"), false); });
  const global = [...host.querySelectorAll(".training-row button")][0] as HTMLButtonElement;
  await act(async () => { global.click(); global.click(); });
  expect(confirm).toHaveBeenCalledTimes(1);
  expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-busy")).toBe("true");
  expect(reload).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
  await act(async () => { pending.reject(new Error("暂时无法写入")); });
  expect(host.querySelector('[role="alert"]')).not.toBeNull();
  expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-busy")).toBe("false");
  expect(reload).not.toHaveBeenCalled();
  await act(async () => { global.click(); });
  expect(confirm).toHaveBeenLastCalledWith(expect.objectContaining({ articleId: 1, dislike: true, feedScope: false }));
  expect(onClose).toHaveBeenCalledOnce();
  expect(reload).toHaveBeenCalledOnce();
  expect(notify).not.toHaveBeenCalled();
});

it("retains successful feedback when feature extraction fails, but never reports success on failed rating", async () => {
  const { ref, notify, reload, onOpen } = setup();
  vi.mocked(api.training).mockRejectedValue(new Error("无法提取"));
  await act(async () => { expect(await ref.current!.rate(focusRow(1, "甲文"), true)).toBe(true); });
  expect(notify).toHaveBeenCalledWith({ message: "已记录偏好", holdMs: 2500 });
  expect(onOpen).not.toHaveBeenCalled();
  expect(reload).toHaveBeenCalledOnce();
  vi.mocked(api.rate).mockRejectedValue(new Error("写入失败"));
  notify.mockClear();
  await act(async () => { expect(await ref.current!.rate(focusRow(1, "甲文"), true)).toBe(false); });
  expect(notify).toHaveBeenCalledOnce();
  expect(notify.mock.calls[0][0]).not.toHaveProperty("holdMs");
  expect(api.training).toHaveBeenCalledOnce();
  expect(reload).toHaveBeenCalledOnce();
});

it("submits a later rating of the same article only after the earlier one is stored", async () => {
  const { ref } = setup();
  // 宿主可能反序完成：第一次评分等写锁，第二次立即写入；不串行时最终会是先点的赞。
  const stored = new Map<number, boolean>();
  const first = deferred<void>();
  vi.mocked(api.rate)
    .mockImplementationOnce(async (id, up) => { await first.promise; stored.set(id, up); })
    .mockImplementation(async (id, up) => { stored.set(id, up); });
  let liked!: Promise<boolean>, disliked!: Promise<boolean>;
  act(() => {
    liked = ref.current!.rate(focusRow(1, "甲文"), true);
    disliked = ref.current!.rate(focusRow(1, "甲文"), false);
  });
  await act(async () => { first.resolve(); await liked; await disliked; });
  expect(stored.get(1)).toBe(false);
});
