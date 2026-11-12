# 块渲染探针

迭代阅读排版时走这里，不要开整软件。图、emoji、内联样式的下载 / 字体 / 控件问题会和壳叠在一起，探针把它们拆开。

## 两层

| 层 | 入口 | 验什么 |
| --- | --- | --- |
| IR | `cargo test -p rrss-render --test article_fixture` | fixture HTML → 块：标题、段、`<img>`、emoji 文本 |
| 像素 | `apps/desktop` `/#render` + Playwright headless | 同一套块用 React `BlockView` 画；图走自定义协议或占位 |

`article.html` 是 IR 的源。`apps/desktop/src/components/BlockView.tsx` 里的 `LIBRARY_FIXTURE` 是它的视觉孪生（块顺序和文案对齐）。改解析器先跑单测；改组件 / 字号 / 段距 / 字体先截图。

## 怎么读截图

- **图能显示**：`<img>` 走到自定义协议或 fixture 占位。App 里仍是「打开原图」→ 查下载、UA，以及缓存是否按魔数解出 PNG / JPEG / WebP（不要只看 URL 后缀）。
- **图变成「图片：…」卡片**：协议没挂上或 `media` 映射为空。先修探针，再谈 App。
- **顶部对照行有粗体 / 斜体 / 链接**：IR 内联 `s` / `e` / `a` 由 React 组件画出，不要把 markdown 星号当字面量塞进文本节点。
- **📚 🎉 ⭐ 是彩色字形**：字体栈够用。
- **📚 🎉 ⭐ 是方框或空白**：缺 emoji 回退。阅读区走 CSS 字体栈；不要往仓库塞 Color Emoji 字体文件。本机 `fc-list` 若没有 Noto Color Emoji，对照会是方框，装上即可。

亮色默认；暗色把 `html[data-theme=dark]` 打开后再截 `/#render`。

```sh
cd apps/desktop
pnpm test:e2e
```

探针页 `/#probe` 另测 CJK 换行、IME 输入框与协议 URL。

## 不要把探针接回 App

不要在这里调真实抓取。探针过了再回阅读叠层查下载与划词。禁止 `dangerouslySetInnerHTML`。
