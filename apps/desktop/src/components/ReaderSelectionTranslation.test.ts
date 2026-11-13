import { deferred } from "../test/deferred";
import { afterEach, expect, it } from "vitest";
import type { SelectionTranslation } from "../bindings";
import {
  SELECTION_TRANSLATION_LIMIT,
  hasSelectionTranslation,
  rememberSelectionTranslation,
  resetSelectionTranslations,
  selectionTranslationCount,
} from "./ReaderSelectionTranslation";

const sample = { text: "译文", config: { identity: "cfg", service: "服务" } } satisfies SelectionTranslation;

afterEach(() => { resetSelectionTranslations(); });

it("drops the oldest finished selection and keeps requests that are still running", async () => {
  for (let i = 0; i < SELECTION_TRANSLATION_LIMIT; i += 1) {
    await rememberSelectionTranslation(`done-${i}`, () => Promise.resolve(sample));
  }
  await rememberSelectionTranslation("done-new", () => Promise.resolve(sample));
  expect(selectionTranslationCount()).toBe(SELECTION_TRANSLATION_LIMIT);
  expect(hasSelectionTranslation("done-0")).toBe(false);
  expect(hasSelectionTranslation("done-new")).toBe(true);

  resetSelectionTranslations();
  const oldest = deferred<SelectionTranslation>();
  rememberSelectionTranslation("oldest", () => oldest.promise);
  for (let i = 0; i < SELECTION_TRANSLATION_LIMIT; i += 1) {
    rememberSelectionTranslation(`pending-${i}`, () => new Promise(() => {}));
  }
  expect(selectionTranslationCount()).toBe(SELECTION_TRANSLATION_LIMIT + 1);
  expect(hasSelectionTranslation("oldest")).toBe(true);
  oldest.resolve(sample);
  await Promise.resolve();
  await Promise.resolve();
  expect(hasSelectionTranslation("oldest")).toBe(false);
  expect(hasSelectionTranslation("pending-0")).toBe(true);
  expect(selectionTranslationCount()).toBe(SELECTION_TRANSLATION_LIMIT);
});
