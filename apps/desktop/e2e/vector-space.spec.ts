import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";

const space = { id: 1, model: "nomic-embed-text", provider_name: "本机 Ollama", dims: 768, source_dims: 768, chunks: 52904, checked_at: null };

async function openVectors(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "model").click();
  return settings.locator("#vector-space");
}

test("a rebuild shows progress and can be cancelled after confirming", async ({ page }) => {
  await installMockHost(page, { vectorSpace: {
    effective: { ...space, state: "active", stale_reason: null },
    building: { space: { ...space, id: 2, model: "bge-m3", dims: 1024, source_dims: 1024, state: "building", chunks: 0, stale_reason: null }, written: 18400, total: 52904 },
  } });
  const vectors = await openVectors(page);
  await expect(vectors.getByRole("status")).toContainText("正在用 bge-m3 重建向量库：18,400 / 52,904 段");
  await expect(vectors.getByRole("button", { name: "向量维度", exact: true })).toBeDisabled();
  await expect(vectors.getByRole("progressbar", { name: "向量库重建进度" })).toBeVisible();
  await vectors.getByRole("button", { name: "取消重建", exact: true }).click();
  await page.getByRole("alertdialog", { name: "取消重建" }).getByRole("button", { name: "取消重建", exact: true }).click();
  await expect(vectors.getByRole("progressbar")).toHaveCount(0);
  await expect(vectors).toContainText("已取消重建。");
});

test("a changed embedding service pauses writes until the original setup is restored", async ({ page }) => {
  await installMockHost(page, { vectorSpace: {
    effective: { ...space, state: "stale", stale_reason: "服务返回的向量和建库时不同，可能是模型版本或量化变了。" },
  } });
  const vectors = await openVectors(page);
  const alert = vectors.getByRole("alert");
  await expect(alert).toContainText("已暂停生成新向量");
  await expect(vectors.getByRole("button", { name: "向量维度", exact: true })).toBeDisabled();
  await alert.getByRole("button", { name: "恢复原配置", exact: true }).click();
  await expect(vectors.getByRole("alert")).toHaveCount(0);
  await expect(vectors).toContainText("已恢复原配置并重新核对。");
  await expect(vectors).toContainText("上次核对");
});

test("dimensions distinguish local truncation from regenerating vectors", async ({ page }) => {
  await installMockHost(page, { vectorSpace: {
    effective: { ...space, dims: 1024, source_dims: 1536, state: "active", stale_reason: null },
    dims_setting: "auto", local: false,
  } });
  const vectors = await openVectors(page);
  const dims = vectors.getByRole("button", { name: "向量维度", exact: true });
  const lastValue = () => page.evaluate(() => (window as any).__mockHost.args.set_vector_dims?.value);
  await expect(vectors).toContainText("1024 维（原始 1536 维）");
  await expect(dims).toHaveText("自动（不超过 1024 维）");
  await dims.click();
  await expect(page.getByRole("menuitemradio", { name: "2048 维", exact: true })).toHaveCount(0);
  await page.getByRole("menuitemradio", { name: "512 维", exact: true }).click();
  const confirm = page.getByRole("alertdialog", { name: "调整向量维度" });
  await expect(confirm).toContainText("直接在本地截断已有的 52,904 段向量，不调用接口、不产生费用");
  await expect(confirm).toContainText("完成前检索继续用当前 1024 维");
  expect(await lastValue()).toBeUndefined();
  await confirm.getByRole("button", { name: "开始调整", exact: true }).click();
  await expect(vectors.getByRole("status").filter({ hasText: "正在把向量库调整到 512 维" })).toContainText("0 / 52,904 段");
  await expect(vectors.getByRole("progressbar", { name: "向量库重建进度" })).toBeVisible();
  await expect(dims).toBeDisabled();
  await expect(dims).toHaveText("512 维");
  expect(await lastValue()).toBe("512");

  await vectors.getByRole("button", { name: "取消重建", exact: true }).click();
  const cancel = page.getByRole("alertdialog", { name: "取消重建" });
  await expect(cancel).toContainText("维度改回 1024 维");
  await cancel.getByRole("button", { name: "取消重建", exact: true }).click();
  await expect(dims).toBeEnabled();
  await expect(dims).toHaveText("1024 维");

  // auto 的目标等于当前维度，直接保存，不弹确认。
  await dims.click();
  await page.getByRole("menuitemradio", { name: "自动（不超过 1024 维）", exact: true }).click();
  await expect(dims).toHaveText("自动（不超过 1024 维）");
  expect(await lastValue()).toBe("auto");
  await expect(confirm).toHaveCount(0);

  await dims.click();
  await page.getByRole("menuitemradio", { name: "原始（1536 维）", exact: true }).click();
  await expect(confirm).toContainText("需要重新生成向量");
  await expect(confirm).toContainText("约 10,580,800 token，按服务商价格计费");
  await expect(confirm).toContainText("重建完成前检索继续用当前 1024 维");
  await confirm.getByRole("button", { name: "取消", exact: true }).click();
  expect(await lastValue()).toBe("auto");
  await expect(dims).toHaveText("自动（不超过 1024 维）");
});
