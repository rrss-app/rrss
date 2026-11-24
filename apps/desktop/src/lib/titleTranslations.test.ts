import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { TITLE_TRANSLATE_BATCH, type TitleTranslations } from "../bindings";
import { createTitleTranslations } from "./titleTranslations";

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

function setup(reply: (ids: number[]) => Promise<TitleTranslations>) {
  let time = 0;
  const send = vi.fn(reply);
  const store = createTitleTranslations(send, () => time);
  return { store, send, advance: (ms: number) => { time += ms; } };
}

const settle = async () => { await vi.advanceTimersByTimeAsync(200); };

it("batches requests, keeps one batch in flight and never asks twice", async () => {
  const { store, send } = setup(async (ids) => ({ titles: ids.map((id) => ({ id, title: `T${id}` })) }));
  const ids = Array.from({ length: TITLE_TRANSLATE_BATCH + 3 }, (_, i) => i + 1);
  for (const id of ids) store.request(id);
  store.request(1);
  await settle();
  await settle();
  expect(send).toHaveBeenCalledTimes(2);
  expect(send.mock.calls[0][0]).toHaveLength(TITLE_TRANSLATE_BATCH);
  expect(send.mock.calls[1][0]).toEqual(ids.slice(TITLE_TRANSLATE_BATCH));
  expect(store.get(1)).toBe("T1");
  store.request(1);
  await settle();
  expect(send).toHaveBeenCalledTimes(2);
});

it("does not retry failures in this run, but a reset asks again", async () => {
  const { store, send } = setup(async () => { throw new Error("service down"); });
  store.request(7);
  await settle();
  store.request(7);
  await settle();
  expect(send).toHaveBeenCalledTimes(1);
  store.reset();
  store.request(7);
  await settle();
  expect(send).toHaveBeenCalledTimes(2);
});

it("pauses after the backend says so and keeps titles it already had", async () => {
  const { store, send, advance } = setup(async () => ({ titles: [{ id: 1, title: "已存" }], paused: "budget" }));
  store.request(1);
  store.request(2);
  await settle();
  expect(store.get(1)).toBe("已存");
  store.request(2);
  await settle();
  expect(send).toHaveBeenCalledTimes(1);
  advance(11 * 60_000);
  store.request(2);
  await settle();
  expect(send).toHaveBeenCalledTimes(2);
});
