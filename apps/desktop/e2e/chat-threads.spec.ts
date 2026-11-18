import { expect, test } from "./fixtures";
import { installMockHost, MOCK_BINDINGS, MOCK_MESSAGES, MOCK_THREADS } from "./mockHost";
import { shot } from "./shots";

// 会话文件夹与归档（阶段 14 · 会话文件夹与归档）：文件夹、移动、置顶、重命名、归档与撤销、已归档列表、删除确认。
const OUT = "../../test-results/chat-threads";

test("a confirmed AI title cannot replace a rename made while it is running", async ({ page }) => {
  await installMockHost(page, {
    systemLanguage: "en",
    threads: [{ ...MOCK_THREADS[0]!, title: "Garden volunteers" }],
    messages: { 7: [{ ...MOCK_MESSAGES[7]![0]!, text: "How should we schedule garden volunteers?", cite_article_id: 0, citations: [] }] },
    bindings: { ...MOCK_BINDINGS, tasks: MOCK_BINDINGS.tasks.map(task => task.task === "chat_title"
      ? { ...task, provider_id: 1, model: "llama3.2" } : task) },
  });
  await page.goto("/");
  await page.evaluate(() => { (window as any).__mockHost.holdTitle = true; });
  const thread = page.getByRole("button", { name: "Garden volunteers", exact: true });
  const openMenu = () => thread.click({ button: "right" });
  await openMenu();
  await page.getByRole("menuitem", { name: "Generate title with AI" }).click();
  await page.getByRole("alertdialog", { name: "Generate a chat title with AI?" }).getByRole("button", { name: "Generate", exact: true }).click();
  await expect(thread).toHaveAttribute("aria-busy", "true");
  await openMenu();
  await expect(page.getByRole("menuitem", { name: "Generate title with AI" })).toBeDisabled();
  await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
  const input = page.getByRole("textbox", { name: "Chat title" });
  await input.fill("Weekend garden rota");
  await input.press("Enter");
  await expect(page.getByRole("button", { name: "Weekend garden rota", exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).__mockHost.releaseTitle(7, "Late garden title"));
  await expect(page.getByRole("button", { name: "Weekend garden rota", exact: true })).not.toHaveAttribute("aria-busy", "true");
  await expect(page.getByRole("button", { name: "Late garden title", exact: true })).toHaveCount(0);
});

test("threads can be filed, pinned, renamed, archived and deleted from the sidebar", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  const sidebar = page.locator("aside.sidebar");
  const row = (title: string) => sidebar.locator(".thread-row").filter({ has: page.getByRole("button", { name: title, exact: true }) });
  const menuFor = async (title: string) => {
    await row(title).hover();
    await sidebar.getByRole("button", { name: `${title} 的操作`, exact: true }).click();
    return page.getByRole("menu", { name: "会话操作" });
  };

  // 新建文件夹，把一条会话移进去。
  await sidebar.getByRole("button", { name: "新建文件夹", exact: true }).click();
  await sidebar.getByLabel("新文件夹名称").fill("工作");
  await sidebar.getByLabel("新文件夹名称").press("Enter");
  const folder = sidebar.getByRole("group", { name: "工作" });
  await expect(folder.getByRole("button", { name: /工作/ })).toHaveAttribute("aria-expanded", "true");
  let menu = await menuFor("Tauri IPC 线程模型");
  await menu.getByRole("menuitem", { name: "移到文件夹…" }).click();
  await page.getByRole("menu", { name: "会话操作" }).getByRole("menuitem", { name: "工作" }).click();
  await expect(folder.getByRole("button", { name: "Tauri IPC 线程模型", exact: true })).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "已移到「工作」" })).toBeVisible();

  // 置顶，并就地重命名。
  menu = await menuFor("查证 Rust 1.97 的回归修复");
  await menu.getByRole("menuitem", { name: "置顶" }).click();
  await expect(row("查证 Rust 1.97 的回归修复").locator(".thread-pin")).toBeVisible();
  menu = await menuFor("查证 Rust 1.97 的回归修复");
  await menu.getByRole("menuitem", { name: "重命名" }).click();
  const rename = sidebar.getByLabel("会话标题");
  await rename.fill("Rust 1.97 回归");
  await rename.press("Enter");
  await expect(sidebar.getByRole("button", { name: "Rust 1.97 回归", exact: true })).toBeVisible();
  await shot(page, `${OUT}/sidebar-folders.png`);

  // 归档：提示条可撤销；再归档后从「已归档」打开，对话页提示已归档。
  menu = await menuFor("Rust 1.97 回归");
  await menu.getByRole("menuitem", { name: "归档" }).click();
  await expect(row("Rust 1.97 回归")).toHaveCount(0);
  await page.getByRole("button", { name: "撤销", exact: true }).click();
  await expect(sidebar.getByRole("button", { name: "Rust 1.97 回归", exact: true })).toBeVisible();
  menu = await menuFor("Rust 1.97 回归");
  await menu.getByRole("menuitem", { name: "归档" }).click();
  await sidebar.getByRole("button", { name: /已归档\s*1/ }).click();
  const archive = page.getByRole("complementary", { name: "已归档的会话" });
  await archive.getByRole("button", { name: "Rust 1.97 回归", exact: true }).click();
  await expect(page.locator(".chat-archived")).toContainText("这条会话已归档");
  await shot(page, `${OUT}/archived.png`);
  await page.locator(".chat-archived").getByRole("button", { name: "取消归档", exact: true }).click();
  await expect(page.locator(".chat-archived")).toHaveCount(0);
  await archive.getByRole("button", { name: "会话", exact: true }).click();
  await expect(sidebar.getByRole("button", { name: "Rust 1.97 回归", exact: true })).toBeVisible();

  // 删除要在菜单里再确认一次；删文件夹不删里面的会话。
  menu = await menuFor("Rust 1.97 回归");
  await menu.getByRole("menuitem", { name: "删除…" }).click();
  const confirm = page.getByRole("dialog", { name: "确认删除" });
  await expect(confirm).toContainText("消息和 Pi 会话记录一起删掉");
  await confirm.getByRole("button", { name: "删除", exact: true }).click();
  await expect(sidebar.getByRole("button", { name: "Rust 1.97 回归", exact: true })).toHaveCount(0);
  await folder.getByRole("button", { name: /工作/ }).hover();
  await sidebar.getByRole("button", { name: "工作 文件夹的操作", exact: true }).click();
  await page.getByRole("menu", { name: "文件夹操作" }).getByRole("menuitem", { name: "删除文件夹…" }).click();
  await expect(page.getByRole("dialog", { name: "确认删除" })).toContainText("里面的 1 条会话回到未归类");
  await page.getByRole("dialog", { name: "确认删除" }).getByRole("button", { name: "删除文件夹", exact: true }).click();
  await expect(sidebar.getByRole("group", { name: "工作" })).toHaveCount(0);
  await expect(sidebar.getByRole("button", { name: "Tauri IPC 线程模型", exact: true })).toBeVisible();
});
