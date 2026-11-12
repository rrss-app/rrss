import type { Page } from "@playwright/test";

/**
 * 曝光要持续可见 1 秒才登记，登记后 5 秒批量写入。用例先 `page.clock.install()`，再用这里拨过两段时间，不真等。
 * 两步分开拨：批量写入的定时器在停留到期后才排上。
 */
export async function settleExposures(page: Page) {
  await page.clock.fastForward(1100);
  await page.clock.fastForward(5100);
}

/**
 * 给 `expect.poll` 用：每轮先拨钟再读。可见性回调可能晚于某一次拨钟才到，
 * 只拨一次会让写入落到真实时间里，与轮询超时赛跑。
 */
export function settled<T>(page: Page, read: () => Promise<T>) {
  return async () => {
    await settleExposures(page);
    return read();
  };
}
