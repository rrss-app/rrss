import { expect, test } from "./fixtures";
import { PREVIEW_FEEDS, PREVIEW_FOCUS } from "../src/lib/preview";
import type { CitationView, DigestSourceView, DigestView } from "../src/bindings";
import { shot } from "./shots";

const firstSource: DigestSourceView = { article_id: 1, title: "本地检索更新", feed_title: "测试源", number: 1, summary: null, published_at: null, status: "available" };
const secondSource: DigestSourceView = { article_id: 2, title: "阅读器性能改善", feed_title: "测试源", number: 2, summary: null, published_at: null, status: "available" };

const fixtures: DigestView[] = [
  { id: 101, period: "daily", period_label: "日报", date_label: "9月16日", period_start: 3,
    body: "### 今日简报\n\n- 本地检索更新：全文检索支持按订阅源筛选，结果仍保留可解释的排序原因。\n- 阅读器性能改善：长文按需加载图片，保留原文与翻译之间的段落位置。",
    revision: "e2e-digest-revision-101",
    source_state: "verified",
    source_note: "",
    source_index: [firstSource, secondSource],
    unlocated_sources: [],
    items: [
      { block_id: 1, sources: [firstSource] },
      { block_id: 2, sources: [secondSource] },
    ],
    document: { v: 1, blocks: [
      { t: "h", level: 3, inlines: [{ t: "t", text: "今日简报" }] },
      { t: "li", marker: "•", indent: 0, inlines: [{ t: "t", text: "本地检索更新：全文检索支持按订阅源筛选，结果仍保留可解释的排序原因。" }] },
      { t: "li", marker: "•", indent: 0, inlines: [{ t: "t", text: "阅读器性能改善：长文按需加载图片，保留原文与翻译之间的段落位置。" }] },
    ] } },
  { id: 100, period: "daily", period_label: "日报", date_label: "9月15日", period_start: 2,
    body: "### 昨日简报",
    revision: "e2e-digest-revision-100",
    source_state: "unavailable",
    source_note: "",
    source_index: [],
    unlocated_sources: [],
    items: [],
    document: { v: 1, blocks: [{ t: "h", level: 3, inlines: [{ t: "t", text: "昨日简报" }] }] } },
];

// 真实 React + Tauri IPC 边界模拟。不会启动 Pi、请求模型或使用日常数据库。
test.beforeEach(async ({ page }) => {
  await page.addInitScript(({ digests, focus, feeds }) => {
    type Message = { id: number; role: string; text: string; streaming: boolean; cite_article_id: number; cite_title: string; citations: CitationView[]; document: unknown };
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const threads = new Map<number, number>();
    const messages = new Map<number, Message[]>();
    let callbackId = 0;
    let messageId = 0;
    const emit = (name: string, payload: unknown) => {
      for (const id of listeners.get(name) ?? []) callbacks.get(id)?.({ event: name, id, payload });
    };
    const msg = (role: string, text: string, document: unknown = null, citations: CitationView[] = []): Message => ({ id: ++messageId, role, text, document, citations, streaming: false, cite_article_id: 0, cite_title: "" });
    const harness = {
      sent: [] as Array<{ threadId: number; digestId: number; text: string }>,
      finish(digestId: number) {
        const id = threads.get(digestId)!;
        const text = "第二条讨论阅读器按需加载。请查看原文核对具体实现。";
        const document = { v: 1, blocks: [{ t: "p", inlines: [
          { t: "t", text: text + " " },
          { t: "a", href: "rrss://article/2", children: [{ t: "t", text: "阅读器性能改善" }] },
        ] }] };
        emit("chat:delta", { thread_id: id, text: "第二条", document: null });
        emit("chat:delta", { thread_id: id, text, document });
        messages.get(id)!.push(msg("assistant", text, document));
        emit("chat:done", id);
      },
    };
    Object.assign(window, {
      __digestTest: harness,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (name: string, id: number) => listeners.get(name)?.delete(id) },
      __TAURI_INTERNALS__: {
        transformCallback(cb: (event: unknown) => void) { callbacks.set(++callbackId, cb); return callbackId; },
        async invoke(command: string, args: Record<string, any> = {}) {
          switch (command) {
            case "plugin:event|listen": {
              const ids = listeners.get(args.event) ?? new Set<number>();
              ids.add(args.handler); listeners.set(args.event, ids); return args.handler;
            }
            case "plugin:event|unlisten": return;
            case "focus_stream": return focus;
            case "feed_tree": return feeds;
            case "threads": return [];
            case "digest_list": return digests.filter((d) => d.period === args.period).map((d) => ({ id: d.id, period: d.period, period_label: d.period_label, period_start: d.period_start, date_label: d.date_label }));
            case "digest": {
              const row = digests.find((d) => d.id === args.id);
              if (!row) throw new Error("digest not found");
              return row;
            }
            case "digest_thread": {
              const id = threads.get(args.digestId) ?? 0;
              return { thread_id: id, messages: messages.get(id) ?? [], has_more: false };
            }
            case "messages": return { messages: messages.get(args.threadId) ?? [], has_more: false };
            case "cite_digest_selection": {
              if (args.digestId !== 101 || args.revision !== digests[0].revision || args.selections.length !== 1 || args.selections[0].block_id !== 2) throw new Error("wrong selected digest revision/block");
              return [{ kind: "digest", article_id: 2, title: "阅读器性能改善", feed_title: "测试源", url: "https://example.test/article/2", block_id: -1, excerpt: args.selections[0].excerpt }];
            }
            case "send_message": {
              const input = args.args;
              if (!digests.some((d) => d.id === input.digestId)) throw new Error("wrong digest");
              harness.sent.push(input);
              const id = threads.get(input.digestId) ?? (input.digestId + 200);
              threads.set(input.digestId, id);
              const rows = messages.get(id) ?? [];
              rows.push(msg("user", input.text, null, input.citations)); messages.set(id, rows);
              emit("chat:start", id);
              return id;
            }
            case "article_reader":
            case "open_article": return {
              id: args.id, title: "阅读器性能改善", meta: "测试源 · 测试数据", url: "https://example.test/article/2", summary: "",
              score: 0, feedback: 0, reasons: [], media: [], translated_title: "", translated_document: null,
              translation_ready: false, translation_note: "", truncated: false, embed_covers: [],
              document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "这是第二条对应的本地原文，用于核对页内追问的返回路径。" }] }] },
            };
            default: throw new Error(`unexpected command: ${command}`);
          }
        },
      },
    });
  }, { digests: fixtures, focus: PREVIEW_FOCUS, feeds: PREVIEW_FEEDS });
});

test("Digest selection keeps scoped drafts, follows up in place and opens sources", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.getByRole("heading", { name: "今日简报" })).toBeVisible();
  const selectedText = page.locator('.digest-doc [data-block-id="2"] span:not([data-selection-ignore])').first();
  const bounds = await selectedText.boundingBox();
  expect(bounds).toBeTruthy();
  await page.mouse.move(bounds!.x + 1, bounds!.y + 8);
  await page.mouse.down();
  await page.mouse.move(bounds!.x + 192, bounds!.y + 8, { steps: 12 });
  await page.mouse.up();
  await expect(page.getByRole("menuitem", { name: "引用", exact: true })).toBeVisible();
  await expect(page.locator(".composer .citation-chip")).toHaveCount(0);
  await page.getByRole("menuitem", { name: "引用", exact: true }).click();
  await expect(page.locator(".composer .citation-excerpt")).toContainText("阅读器性能改善");
  await expect(page.getByRole("textbox", { name: "追问当前简报" })).toHaveValue("");
  await page.getByRole("textbox", { name: "追问当前简报" }).fill("展开第二条");
  await page.getByRole("button", { name: "上期", exact: true }).click();
  await expect(page.locator(".composer .citation-chip")).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "追问当前简报" })).toHaveValue("");
  await page.getByRole("textbox", { name: "追问当前简报" }).fill("另一份草稿");
  await page.getByRole("button", { name: "下期", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "追问当前简报" })).toHaveValue("展开第二条");
  await expect(page.locator(".composer .citation-chip")).toHaveCount(1);
  await shot(page, "e2e/screenshots/digest-selection-composer.png");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("button", { name: "停止生成" })).toBeVisible();
  await expect(page.locator(".digest-discussion .user .message-body > p")).toHaveText("展开第二条");
  await expect(page.locator(".digest-discussion .citation-excerpt")).toContainText("阅读器性能改善");
  await page.getByRole("button", { name: "上期", exact: true }).click();
  await expect(page.getByRole("heading", { name: "昨日简报" })).toBeVisible();
  await page.evaluate(() => (window as any).__digestTest.finish(101));
  await expect(page.locator(".digest-discussion")).toHaveCount(0);
  await page.getByRole("button", { name: "下期", exact: true }).click();
  await expect(page.locator(".digest-discussion .assistant")).toContainText("第二条讨论阅读器按需加载。");
  await expect(page.locator(".chat-pane")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__digestTest.sent[0])).toMatchObject({ threadId: 0, digestId: 101, text: "展开第二条", citations: [{ kind: "digest", article_id: 2, block_id: -1 }] });
  await shot(page, "e2e/screenshots/digest-selection-light.png", { fullPage: true });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  await expect(page.getByRole("button", { name: "简报", exact: true })).toHaveCSS("color", "rgb(236, 234, 230)");
  await shot(page, "e2e/screenshots/digest-selection-dark.png", { fullPage: true });
  await page.locator(".digest-discussion").getByRole("link", { name: "阅读器性能改善" }).click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.getByRole("button", { name: "返回", exact: true }).click();
  await expect(page.locator(".drawer.reader")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "今日简报" })).toBeVisible();
  await page.getByRole("textbox", { name: "追问当前简报" }).fill("继续解释");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__digestTest.sent[1])).toMatchObject({ threadId: 301, digestId: 101 });
  await page.evaluate(() => (window as any).__digestTest.finish(101));
});
