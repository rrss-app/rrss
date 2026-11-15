import assert from "node:assert/strict";
import { test } from "node:test";
import { compareLocale, flatten, onlyNamespaces } from "./check-locales.mjs";

const messages = (ns, tree) => flatten(ns, tree);

test("a complete translation passes, with English plural forms for a Chinese _other key", () => {
  const source = messages("focus", { title: "今日焦点", items_other: "{{count}} 篇", fail: "保存失败：", link: "点<b>这里</b>" });
  const target = messages("focus", { title: "Today", items_one: "1 article", items_other: "{{count}} articles", fail: "Couldn't save:", link: "Click <b>here</b>" });
  assert.deepEqual(compareLocale("en", source, target), []);
});

test("missing keys, plural forms, variables, tags, colons and Chinese are reported", () => {
  const source = messages("chat", { a: "甲", b_other: "{{count}} 条", c: "{{name}} 你好", d: "<b>粗</b>", e: "失败：", f: "乙" });
  const target = messages("chat", { b_other: "{{count}} items", c: "Hello {{who}}", d: "bold", e: "Failed", f: "乙", extra: "x" });
  assert.deepEqual(compareLocale("en", source, target).sort(), [
    "chat:a: missing",
    "chat:b_one: missing plural form",
    "chat:c: missing {{name}}",
    "chat:c: unknown {{who}}",
    "chat:d: tags differ from source",
    "chat:e: prefix must end with a colon",
    "chat:extra: not in source",
    "chat:f: contains Chinese",
  ]);
});

test("source-only date templates may be left out", () => {
  const source = messages("shell", { time: { monthDay: "{{month}}月{{day}}日", today: "今天" } });
  assert.deepEqual(compareLocale("en", source, messages("shell", { time: { today: "Today" } })), []);
});

test("partial checks keep only the namespaces the translation already has", () => {
  const source = new Map([...messages("search", { a: "甲" }), ...messages("focus", { b: "乙" })]);
  const target = messages("search", { a: "A" });
  assert.deepEqual([...onlyNamespaces(source, target).keys()], ["search:a"]);
  assert.deepEqual(compareLocale("en", onlyNamespaces(source, target), target), []);
});

test("partial checks also work on a few top-level groups of a large namespace", () => {
  const source = messages("errors", { a: "甲", b_other: "{{count}} 条", c: { d: "丙" }, e: "丁" });
  const target = messages("errors", { a: "A", b_one: "1 item", b_other: "{{count}} items", c: { d: "C" } });
  assert.deepEqual([...onlyNamespaces(source, target).keys()], ["errors:a", "errors:b_other", "errors:c.d"]);
  assert.deepEqual(compareLocale("en", onlyNamespaces(source, target), target), []);
});
