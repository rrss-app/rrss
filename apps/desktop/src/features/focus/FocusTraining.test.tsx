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
  vi.spyOn(api, "rate").mockImplementation(async (_id, up) => ({ up, previous: 0, previous_at: 0, suggested_rule_id: null }));
  vi.spyOn(api, "training").mockResolvedValue(trainingFeaturesFixture());
  const confirm = vi.spyOn(api, "confirmFeature").mockResolvedValue(0);
  const onOpen = vi.fn(), onClose = vi.fn(), notify = vi.fn(), onFeedback = vi.fn();
  const ref = createRef<FocusTrainingHandle>();
  const { host } = harness.mount(<FocusTraining ref={ref} session={session} ratings={createArticleWriteOrder()} onFeedback={onFeedback} open style={{}} onOpen={onOpen} onClose={onClose} notify={notify} />);
  return { reload, confirm, onOpen, onClose, notify, onFeedback, ref, host };
}

it("serializes confirmation and refreshes only after a successful save, retaining failed suggestions", async () => {
  const { ref, host, reload, confirm, onClose, notify } = setup();
  const pending = deferred<number>();
  confirm.mockReturnValueOnce(pending.promise);
  await act(async () => { await ref.current!.rate(focusRow(1, "甲文"), true); });
  // 赞不自动弹训练条：从提示里的「加一条规则…」打开。
  expect(host.querySelector(".training-row")).toBeNull();
  await act(async () => { notify.mock.calls[0][0].onSecondary(); });
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
  expect(confirm).toHaveBeenLastCalledWith(expect.objectContaining({ articleId: 1, dislike: false, feedScope: false }));
  expect(onClose).toHaveBeenCalledOnce();
  expect(reload).toHaveBeenCalledOnce();
  expect(notify).toHaveBeenCalledOnce();
});

it("a like is kept quietly: undo restores the earlier rating, and rules open only on request", async () => {
  const { ref, notify, reload, onOpen, onFeedback } = setup();
  const undo = vi.spyOn(api, "undoRating").mockResolvedValue(undefined);
  vi.mocked(api.rate).mockResolvedValueOnce({ up: true, previous: -1, previous_at: 42, suggested_rule_id: null });
  const changed = vi.fn();
  await act(async () => { expect(await ref.current!.rate(focusRow(1, "甲文"), true, changed)).toBe(true); });
  expect(api.training).not.toHaveBeenCalled();
  expect(onOpen).not.toHaveBeenCalled();
  expect(onFeedback).toHaveBeenLastCalledWith(1, 1);
  await act(async () => { notify.mock.calls[0][0].onAction(); });
  expect(undo).toHaveBeenCalledWith(1, expect.objectContaining({ up: true, previous: -1, previous_at: 42 }));
  expect(changed).toHaveBeenCalledWith(-1);
  expect(onFeedback).toHaveBeenLastCalledWith(1, -1);
  // 赞不动列表，撤销也不重读。
  expect(reload).not.toHaveBeenCalled();

  // 规则候选读不到时只提示，评分照旧保留。
  notify.mockClear();
  vi.mocked(api.training).mockRejectedValue(new Error("无法提取"));
  await act(async () => { await ref.current!.rate(focusRow(1, "甲文"), true); });
  await act(async () => { notify.mock.calls[0][0].onSecondary(); });
  expect(onOpen).not.toHaveBeenCalled();
  expect(notify).toHaveBeenCalledTimes(2);

  vi.mocked(api.rate).mockRejectedValue(new Error("写入失败"));
  notify.mockClear();
  await act(async () => { expect(await ref.current!.rate(focusRow(1, "甲文"), true)).toBe(false); });
  expect(notify).toHaveBeenCalledOnce();
  expect(notify.mock.calls[0][0]).not.toHaveProperty("holdMs");
});

it("liking a liked article removes the like instead of rating it again", async () => {
  const { ref, notify, onFeedback } = setup();
  const undo = vi.spyOn(api, "undoRating").mockResolvedValue(undefined);
  const changed = vi.fn();
  await act(async () => { expect(await ref.current!.rate({ ...focusRow(1, "甲文"), feedback: 1 }, true, changed)).toBe(false); });
  expect(api.rate).not.toHaveBeenCalled();
  expect(undo).toHaveBeenCalledWith(1, { up: true, previous: 0, previous_at: 0, suggested_rule_id: null });
  expect(changed).toHaveBeenCalledWith(0);
  expect(onFeedback).toHaveBeenCalledWith(1, 0);
  expect(notify.mock.calls[0][0]).not.toHaveProperty("onAction");
});

it("submits a later rating of the same article only after the earlier one is stored", async () => {
  const { ref } = setup();
  // 宿主可能反序完成：第一次评分等写锁，第二次立即写入；不串行时最终会是先点的赞。
  const stored = new Map<number, boolean>();
  const first = deferred<void>();
  vi.mocked(api.rate)
    .mockImplementationOnce(async (id, up) => { await first.promise; stored.set(id, up); return { up, previous: 0, previous_at: 0, suggested_rule_id: null }; })
    .mockImplementation(async (id, up) => { stored.set(id, up); return { up, previous: 0, previous_at: 0, suggested_rule_id: null }; });
  let liked!: Promise<boolean>, disliked!: Promise<boolean>;
  act(() => {
    liked = ref.current!.rate(focusRow(1, "甲文"), true);
    disliked = ref.current!.rate(focusRow(1, "甲文"), false);
  });
  await act(async () => { first.resolve(); await liked; await disliked; });
  expect(stored.get(1)).toBe(false);
});

it("a reader dislike offers undo instead of the training bar, and ranked rules only on request", async () => {
  const { ref, host, notify, confirm, onOpen } = setup();
  const undo = vi.spyOn(api, "undoRating").mockResolvedValue(undefined);
  vi.spyOn(api, "ruleCandidates").mockResolvedValue([
    { dimension: "tag", pattern: "Deals", matched_30d: 3, read_30d: 0, caution: "none", best: true },
    { dimension: "author", pattern: "某某", matched_30d: 9, read_30d: 2, caution: "reads", best: false },
  ]);
  const undone = vi.fn();
  await act(async () => { expect(await ref.current!.rate(focusRow(1, "甲文"), false, undone)).toBe(true); });
  expect(api.training).not.toHaveBeenCalled();
  expect(onOpen).not.toHaveBeenCalled();
  await act(async () => { notify.mock.calls[0][0].onAction(); });
  expect(undo).toHaveBeenCalledWith(1, expect.objectContaining({ up: false }));
  expect(undone).toHaveBeenCalledWith(0);

  notify.mockClear();
  await act(async () => { await ref.current!.rate(focusRow(1, "甲文"), false); });
  await act(async () => { notify.mock.calls[0][0].onSecondary(); });
  expect(onOpen).toHaveBeenCalledOnce();
  // 可能误伤的候选先折叠。
  expect(host.querySelectorAll(".rule-candidate")).toHaveLength(1);
  await act(async () => { (host.querySelector(".rule-candidate button") as HTMLButtonElement).click(); });
  expect(confirm).toHaveBeenLastCalledWith(expect.objectContaining({ articleId: 1, dislike: true, dimension: "tag", pattern: "Deals", feedScope: false }));
});
