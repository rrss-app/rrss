import { defineConfig, devices } from "@playwright/test";
import { PERF_ORIGIN, PERF_PORT } from "./perf/scenes";

const dist = "test-results/f0-2/dist";

export default defineConfig({
  testDir: "./perf",
  testMatch: "baseline.spec.ts",
  outputDir: "./test-results/f0-2/playwright",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  timeout: 20 * 60 * 1000,
  expect: { timeout: 20_000 },
  use: {
    baseURL: PERF_ORIGIN,
    headless: true,
    trace: "off",
    // 预装浏览器与 Playwright 版本不一致时，用 RRSS_CHROMIUM 指定可执行文件。
    launchOptions: process.env.RRSS_CHROMIUM ? { executablePath: process.env.RRSS_CHROMIUM } : {},
    viewport: { width: 1280, height: 800 },
  },
  webServer: {
    command: `pnpm exec vite build --outDir ${dist} --emptyOutDir && pnpm exec vite preview --host 127.0.0.1 --port ${PERF_PORT} --strictPort --outDir ${dist}`,
    url: PERF_ORIGIN,
    reuseExistingServer: false,
    timeout: 180_000,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
