/**
 * 阅读器图片预热：提前把本地媒体（`rrss://`）读进来并解码，切到下一篇 / 退回上一篇时图片直接出现，不再先空后跳。
 * 只留最近的少量图片，解码后的位图由浏览器随这些 Image 对象一起保留；超出上限按最早使用淘汰。
 */
const WARM_LIMIT = 24;
const warmed = new Map<string, HTMLImageElement>();

export function warmImages(hrefs: Iterable<string>) {
  if (typeof Image === "undefined") return;
  for (const href of hrefs) {
    if (!href) continue;
    const hit = warmed.get(href);
    if (hit) {
      // 重新放到队尾：正在看的和马上要看的最后淘汰。
      warmed.delete(href);
      warmed.set(href, hit);
      continue;
    }
    const image = new Image();
    image.decoding = "async";
    image.src = href;
    void image.decode?.().catch(() => {});
    warmed.set(href, image);
    while (warmed.size > WARM_LIMIT) warmed.delete(warmed.keys().next().value!);
  }
}
