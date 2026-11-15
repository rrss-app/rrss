import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";

// 英文界面：系统语言为英文时首帧就是英文；主路径上不出现中文语言包里的界面文字。
// 按语言包原文比对而不是按汉字：模拟宿主里的中文样例标题与正文是内容，不算残留。

const LOCALES = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../src/locales/zh-CN");

/**
 * 中文语言包里的每条原文，转成整段匹配的正则：插值处任意文字，标签去掉。
 * 只认界面上恰好等于一条原文的文字节点或属性，模拟宿主里的中文标题与正文不会误报。
 */
function sourcePatterns(): string[] {
  const out = new Set<string>();
  const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const walk = (value: unknown) => {
    if (typeof value === "string") {
      const parts = value.replace(/<[^>]*>/g, "").split(/\{\{[^}]*\}\}/).map((part) => part.trim());
      // 只有插值、或者插值以外没有中文的原文（如「{{detail}}」）不查。
      const han = (parts.join("").match(/\p{Script=Han}/gu) ?? []).length;
      // 带插值的原文字面部分太短（「{{count}} 行」）时，任何以这个字结尾的中文内容都会命中，不查。
      if (han === 0 || (parts.length > 1 && han < 3)) return;
      out.add(`^${parts.map(escape).join("[\\s\\S]*")}$`);
    } else if (value && typeof value === "object") Object.values(value).forEach(walk);
  };
  for (const file of fs.readdirSync(LOCALES)) walk(JSON.parse(fs.readFileSync(path.join(LOCALES, file), "utf8")));
  // 语言选项用各自的语言写。
  out.delete("^简体中文$");
  return [...out];
}

const PATTERNS = sourcePatterns();
/** 模拟宿主里的中文样例数据，恰好与某条原文同字：文件夹名、正文里的链接文字、记忆的范围、旧格式提示的原文。 */
const MOCK_CONTENT = new Set(["中文", "原文", "全局", "托管 Pi 0.87.1"]);

async function leftovers(page: import("@playwright/test").Page) {
  return page.evaluate(([patterns, mock]) => {
    const shown: string[] = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const parent = node.parentElement;
      if (parent && parent.closest("[hidden],[aria-hidden=true]")) continue;
      const text = (node.nodeValue ?? "").trim();
      if (text) shown.push(text);
    }
    for (const el of document.querySelectorAll("[aria-label],[title],[placeholder]")) {
      for (const name of ["aria-label", "title", "placeholder"]) {
        const value = el.getAttribute(name)?.trim();
        if (value) shown.push(value);
      }
    }
    const regexes = patterns.map((source) => new RegExp(source, "u"));
    return [...new Set(shown.flatMap((text) => {
      if (!/\p{Script=Han}/u.test(text) || mock.includes(text)) return [];
      const hit = regexes.find((re) => re.test(text));
      return hit ? [`${text}  ←  ${hit.source}`] : [];
    }))];
  }, [PATTERNS, [...MOCK_CONTENT]] as const);
}

test("an English system gets an English interface without source-language text on the main paths", async ({ page }) => {
  await installMockHost(page, { systemLanguage: "en" });
  await page.goto("/");
  await expect(page.locator(".focus")).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.lang)).toBe("en");
  const found: Record<string, string[]> = {};
  const check = async (where: string) => {
    const left = await leftovers(page);
    if (left.length) found[where] = left;
  };
  await check("focus");

  await page.getByRole("article").first().click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await check("reader");
  await page.keyboard.press("Escape");

  await page.keyboard.press("Control+k");
  await expect(page.locator(".command-palette")).toBeVisible();
  await check("search");
  await page.keyboard.press("Escape");

  await page.locator(".sidebar-foot button").first().click();
  const settings = page.locator(".settings-drawer");
  await expect(settings).toBeVisible();
  for (const id of ["subscribe", "web", "rules", "memory", "model", "assistant", "translate", "diagnostics", "storage", "advanced"]) {
    await settingsTab(settings, id).click();
    await check(`settings/${id}`);
  }
  await page.keyboard.press("Escape");
  await expect(settings).toBeHidden();

  const nav = page.locator(".sidebar .nav > button.nav-item:not(.nav-column)");
  // 订阅视图会把侧栏换成订阅树，放到最后。
  for (const [index, where] of [[2, "digest"], [3, "chat"], [1, "feeds"]] as const) {
    await nav.nth(index).click();
    await check(where);
  }
  expect(found).toEqual({});
});

test("switching the interface language in settings takes effect at once and back", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  expect(await page.evaluate(() => document.documentElement.lang)).toBe("zh-CN");
  await page.locator(".sidebar-foot button").first().click();
  const foot = page.locator(".settings-nav-foot");
  await foot.getByRole("button", { name: "语言" }).click();
  await page.getByRole("menuitemradio", { name: "English", exact: true }).click();
  await expect.poll(() => page.evaluate(() => document.documentElement.lang)).toBe("en");
  // 除了「简体中文」这个选项名，导航底部已经没有中文。
  await expect.poll(() => foot.evaluate((el) => /\p{Script=Han}/u.test(el.textContent!.replace("简体中文", "")))).toBe(false);
  await foot.getByRole("button", { name: "Language" }).click();
  await page.getByRole("menuitemradio", { name: "简体中文", exact: true }).click();
  await expect.poll(() => page.evaluate(() => document.documentElement.lang)).toBe("zh-CN");
});
