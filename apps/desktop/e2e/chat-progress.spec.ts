import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_THREADS } from "./mockHost";

// 对话页打磨第二批：等待过程（单行）收成摘要。
const emit = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([name, value]) => (window as any).__mockHost.emit(name, value), [event, payload] as const);
const step = (threadId: number, patch: Record<string, unknown>) => ({
  thread_id: threadId, text: "", notice_version: "",
  step: { server: "", server_tool: "", kind: "tool", id: "", start: true, tool: "", label: "", detail: "", article_id: null, error: false, ...patch },
});

test("the reply slot shows one line of progress, then folds it into an expandable summary", async ({ page }) => {
  await installMockHost(page, { holdSend: true });
  await page.goto("/");
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  const pane = page.locator(".chat-pane");
  await page.getByRole("textbox", { name: "对话消息" }).fill("1.97.0 的回归为什么没拦住？");
  await page.getByRole("textbox", { name: "对话消息" }).press("Enter");
  const wait = pane.locator(".chat-wait");
  await expect(wait.locator(".chat-wait-label")).toHaveText("思考");
  // 冷启动：引擎步骤可能早于线程建立。
  await emit(page, "chat:status", step(0, { kind: "engine", id: "engine", label: "启动对话引擎" }));
  await expect(wait.locator(".chat-wait-label")).toHaveText("启动对话引擎");
  await emit(page, "chat:status", step(7, { kind: "engine", id: "engine", label: "启动对话引擎", start: false }));
  await expect(wait.locator(".chat-wait-label")).toHaveText("思考");
  await emit(page, "chat:status", step(7, { id: "c1", tool: "search_articles", label: "查阅本地文章", detail: "“Rust 回归”" }));
  await expect(wait.locator(".chat-wait-step")).toHaveText("查阅本地文章“Rust 回归”");
  await expect(wait.locator(".chat-wait-time")).toContainText("秒");
  // 工具进度不再进输入框上方的过程条。
  await expect(page.locator(".chat-status-line")).toHaveCount(0);
  await emit(page, "chat:status", step(7, { id: "c1", tool: "search_articles", label: "查阅本地文章", start: false }));
  await emit(page, "chat:start", 7);
  await emit(page, "chat:delta", { thread_id: 7, text: "先说结论。", document: null });
  await expect(wait).toHaveCount(0);
  const summary = pane.getByRole("button", { name: /^查阅了 1 处 · 用时 [\d.]+ 秒$/ });
  await expect(summary).toBeVisible();
  await summary.click();
  const rows = pane.locator(".chat-trace-list li");
  await expect(rows.locator(".chat-trace-label")).toContainText(["启动对话引擎", "查阅本地文章"]);
  await expect(rows.filter({ hasText: "查阅本地文章" }).locator(".chat-trace-detail")).toHaveText("“Rust 回归”");
  // 回复落库后摘要仍挂在这条回复上（本次运行内）。
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
  await expect(pane.locator(".bubble.assistant").last()).toContainText("我会结合这些引用继续分析");
  await expect(pane.locator(".chat-turn").last().getByRole("button", { name: /^查阅了 1 处/ })).toBeVisible();
  await expect(pane.locator(".chat-wait")).toHaveCount(0);
});

// 外部工具（内置 MCP）：步骤带服务名，会改文件的先确认，可记到本次对话结束。
test("external tool steps name the service, and a confirm card can remember the answer", async ({ page }) => {
  await installMockHost(page, { holdSend: true });
  await page.goto("/");
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  const pane = page.locator(".chat-pane");
  await page.getByRole("textbox", { name: "对话消息" }).fill("把要点存进笔记");
  await page.getByRole("textbox", { name: "对话消息" }).press("Enter");
  const wait = pane.locator(".chat-wait");
  const mcp = { tool: "mcp__s1__write_file", server: "filesystem", server_tool: "write_file", label: "使用 filesystem 的 write_file" };
  await emit(page, "chat:status", step(7, { id: "m1", tool: "mcp__s1__list_directory", server: "filesystem", server_tool: "list_directory", label: "使用 filesystem 的 list_directory" }));
  await expect(wait.locator(".chat-wait-label")).toHaveText("使用 filesystem 的 list_directory");
  await expect(wait.locator(".chat-wait-label .chat-srv")).toHaveText("filesystem");
  await emit(page, "chat:status", step(7, { id: "m1", tool: "mcp__s1__list_directory", server: "filesystem", server_tool: "list_directory", start: false }));
  await emit(page, "chat:status", step(7, { id: "m2", ...mcp }));
  await emit(page, "chat:confirm", {
    thread_id: 7, id: "ask-1", title: "", message: "", timeout_ms: 300000,
    server: "filesystem", tool: "write_file", args: "path: ~/notes/rust.md\ncontent: \"# Rust 1.97.1\"（共 412 字）", can_remember: true,
  });
  // 等确认时等待行停下，写清在等什么。
  await expect(wait).toHaveAttribute("data-paused", "");
  await expect(wait.locator(".chat-wait-asking")).toHaveText("等你确认：filesystem 的 write_file");
  const card = page.getByRole("alertdialog", { name: "允许使用 filesystem 的 write_file？" });
  await expect(card.locator(".chat-confirm-args")).toContainText("path: ~/notes/rust.md");
  await expect(card).toContainText("5 分钟内不处理会自动拒绝");
  await card.getByRole("checkbox", { name: "本次对话里不再问 write_file" }).check();
  await card.getByRole("button", { name: "允许", exact: true }).click();
  await expect(card).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.confirm_fetch)).toEqual({ id: "ask-1", ok: true, remember: true });
  await expect(wait).not.toHaveAttribute("data-paused", "");
  await emit(page, "chat:status", step(7, { id: "m2", ...mcp, start: false }));
  await emit(page, "chat:start", 7);
  await emit(page, "chat:delta", { thread_id: 7, text: "已经存好。", document: null });
  const summary = pane.getByRole("button", { name: /^用了 2 次外部工具 · 用时 [\d.]+ 秒$/ });
  await expect(summary).toBeVisible();
  await summary.click();
  const rows = pane.locator(".chat-trace-list li");
  await expect(rows.locator(".chat-trace-label")).toContainText(["filesystem · list_directory", "filesystem · write_file"]);
  await expect(rows.filter({ hasText: "write_file" }).locator(".chat-trace-approval")).toHaveText("· 已允许");
  // 出字后又调用工具：回复下方再出现等待过程，这次调用照样计入摘要。
  await emit(page, "chat:status", step(7, { id: "m3", tool: "mcp__s1__read_file", server: "filesystem", server_tool: "read_file", label: "使用 filesystem 的 read_file" }));
  await expect(wait.locator(".chat-wait-label")).toHaveText("使用 filesystem 的 read_file");
  await expect(pane.locator(".bubble.assistant[data-streaming] ~ .chat-wait")).toHaveCount(1);
  await emit(page, "chat:status", step(7, { id: "m3", tool: "mcp__s1__read_file", server: "filesystem", server_tool: "read_file", start: false }));
  await emit(page, "chat:delta", { thread_id: 7, text: "已经存好。核对过内容。", document: null });
  await expect(wait).toHaveCount(0);
  await expect(pane.getByRole("button", { name: /^用了 3 次外部工具 · 用时 [\d.]+ 秒$/ })).toBeVisible();
  await expect(rows.locator(".chat-trace-label")).toContainText(["filesystem · list_directory", "filesystem · write_file", "filesystem · read_file"]);
});

// 对话页打磨第二批：回复里本地来源与外部链接的悬停预览。
const reply = {
  id: 2, role: "assistant", text: "见两篇。", streaming: false, cite_article_id: 0, cite_title: "", citations: [],
  document: { v: 1, blocks: [{ t: "p", inlines: [
    { t: "t", text: "先看 " }, { t: "a", href: "rrss://article/1", children: [{ t: "t", text: "单库取舍" }] },
    { t: "t", text: "，再对照 " }, { t: "a", href: "rrss://article/404", children: [{ t: "t", text: "旧文" }] },
    { t: "t", text: "与 " }, { t: "a", href: "https://github.com/rust-lang/rust/pull/14821", children: [{ t: "t", text: "rust#14821" }] }, { t: "t", text: "。" },
  ] }] },
};
const question = {
  id: 1, role: "user", text: "这两篇怎么看？", streaming: false, cite_article_id: 0, cite_title: "", document: null,
  citations: [{ kind: "article", article_id: 2, title: "Tauri 2 的 IPC 命令默认在主线程执行", feed_title: "Byte Harbor", url: "", excerpt: "", block_id: -1 }],
};

test("hovering local sources previews the article; external links show only their address", async ({ page }) => {
  await installMockHost(page, { messages: { 7: [question, reply] } });
  await page.goto("/");
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  const answer = page.locator(".chat-pane .bubble.assistant");
  const card = page.locator(".chat-link-card");
  const external = answer.getByRole("link", { name: "rust#14821", exact: true });
  await expect(external).toHaveAttribute("data-site", "github.com");
  // 擦过不弹。
  await answer.getByRole("link", { name: "单库取舍" }).hover();
  await page.mouse.move(5, 5);
  await page.waitForTimeout(450);
  await expect(card).toHaveCount(0);
  await answer.getByRole("link", { name: "单库取舍" }).hover();
  const preview = page.getByRole("dialog", { name: "文章预览" });
  await expect(preview).toContainText("sqlite-vec 在桌面端单库里的取舍");
  await expect(preview).toContainText("Compiler Notes");
  await expect(preview.getByLabel("未读")).toBeVisible();
  // 卡片开着时移到另一处立即换内容：已移除的文章按简报来源的说法。
  await answer.getByRole("link", { name: "旧文" }).hover();
  await expect(preview).toContainText("文章已移除");
  await expect(preview.getByRole("button", { name: "引用到输入框" })).toBeDisabled();
  await external.hover();
  const link = page.getByRole("dialog", { name: "链接预览" });
  await expect(link).toContainText("https://github.com/rust-lang/rust/pull/14821");
  await expect(link.getByRole("button", { name: "复制链接" })).toBeVisible();
  // 消息里的引用小签同样能预览；引用到输入框后加进待发引用。
  await page.locator(".chat-pane .bubble.user .citation-chip").hover();
  await expect(preview).toContainText("Tauri 2 的 IPC 命令默认在主线程执行");
  await preview.getByRole("button", { name: "引用到输入框" }).click();
  await expect(page.locator(".composer .cite-row .citation-chip")).toContainText("Tauri 2 的 IPC 命令默认在主线程执行");
  await expect(card).toHaveCount(0);
  // 打开走阅读器。
  await answer.getByRole("link", { name: "单库取舍" }).hover();
  await preview.getByRole("button", { name: /^打开/ }).click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
});
