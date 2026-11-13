import type { Locator, Page } from "@playwright/test";

/** 审阅用截图：默认不生成，需要时 `RRSS_SHOTS=1 pnpm test:e2e e2e/<name>.spec.ts`。 */
export async function shot(target: Page | Locator, path: string, options: { fullPage?: boolean } = {}) {
  if (!process.env.RRSS_SHOTS) return;
  await (target as Page).screenshot({ path, animations: "disabled", ...options });
}
