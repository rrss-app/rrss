# 黄金馈源

阶段 1 起放入订阅快照（RSS / Atom / JSON Feed）和 OPML，供 `rrss-core` / `rrss-render` 回归。正文带常见 HTML，不要手写「完美 Markdown」充当唯一语料。

合成样例用来钉去重 / 标签 / fragment；`real/` 是 2026-08-23 从公网抓的真实快照，版权归原站点，只做解析回归。为减少收录他人原文，只留解析回归需要的条数，正文只留开头：Hacker News 与 Solidot 各留 10 条、LWN 与少数派各留 5 条，Solidot、LWN、少数派每条描述只留第一句；阮一峰日志只留第一篇的前三段；V2EX 只留前 10 条。Hacker News、少数派、V2EX 的作者名换成 `userN` 占位，正文里的个人邮箱换成 `.example` 地址。

| 文件 | 用途 |
| --- | --- |
| `rss2-sample.xml` | RSS 2.0，含 tracking query 与 `<category>` |
| `atom-sample.xml` | Atom，HTML content |
| `jsonfeed-sample.json` | JSON Feed 1.1，permalink 带 fragment |
| `real/hnrss-frontpage.xml` | 真实 RSS 2.0（Hacker News） |
| `real/solidot.xml` | 真实 RSS 2.0，中文 HTML |
| `real/sspai.xml` | 真实 RSS 2.0，中文摘要 |
| `real/lwn.xml` | 真实 RSS 2.0（LWN） |
| `real/ruanyifeng-atom.xml` | 真实 Atom，全文 HTML |
| `real/v2ex-atom.xml` | 真实 Atom，社区帖 |
| `real/rsshub-div-br.xml` | 自建 RSSHub 式 `div`+`<br><br>` description |
| `real/jsonfeed-org.json` | 真实 JSON Feed 1.0 |
| `opml/nested.opml` | 文件夹树 |
| `opml/duplicates.opml` | 同 URL 两次 |
| `opml/real.opml` | 10 个可抓的公网源（中英 + JSON Feed） |

把真实源装进本机库并立刻刷新：

```sh
cargo run -p rrss-core --example import_opml -- testdata/feeds/opml/real.opml
```

或在应用里：订阅 → 导入 OPML → 选该文件 → 刷新。