import { defineConfig, devices } from "@playwright/test";
import { DEV_ID_PATH, DEV_ORIGIN, DEV_PORT } from "./dev-server.config";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  use: {
    baseURL: DEV_ORIGIN,
    headless: true,
    trace: "off",
    // 预装浏览器与 Playwright 版本不一致时，用 RRSS_CHROMIUM 指定可执行文件。
    launchOptions: process.env.RRSS_CHROMIUM ? { executablePath: process.env.RRSS_CHROMIUM } : {},
  },
  webServer: {
    command: `pnpm exec vite --host 127.0.0.1 --port ${DEV_PORT}`,
    // 探测本检出的身份路径：只复用自己的 Vite，端口上是别的服务时报错
    url: `${DEV_ORIGIN}${DEV_ID_PATH}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
