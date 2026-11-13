# 参与开发

感谢关注 RRSS。这份说明讲怎么在本机跑起来、改动该放在哪一层、提交前跑什么检查。架构概览见 [doc/architecture.md](doc/architecture.md)。issue 与 PR 用中文或英文都可以。

## 环境

- Node 22、pnpm 10、Rust stable（1.85 及以上，项目使用 2024 edition）。
- Linux 另需 WebKitGTK 开发库：Debian / Ubuntu 为 `libwebkit2gtk-4.1-dev`。
- 日常开发以 Linux / WSL 为主。Windows 原生开发入口还没有系统验证；Windows 目标的编译检查与发布包由 CI 完成。

前端依赖与锁文件在 `apps/desktop/`，不在仓库根；所有 pnpm 命令都用 `pnpm --dir apps/desktop …`。

## 运行

在仓库根目录：

```sh
pnpm --dir apps/desktop install
pnpm --dir apps/desktop dev:desktop
```

这是 Debug + Vite 热更新：React / CSS 改动即时生效，Rust 改动由 Tauri CLI 重编后重启。开发窗口默认使用和正式版相同的数据目录。要用一份独立的空库（Linux / WSL）：

```sh
RRSS_DEV_ISOLATE=1 pnpm --dir apps/desktop dev:desktop
```

不依赖 Vite、可独立运行的 Debug 构建（产物 `target/debug/rrss`）与发布构建：

```sh
scripts/build-debug.sh
cargo build --release --locked -p rrss-desktop
```

## 改动放在哪

| 位置 | 放什么 |
| :-- | :-- |
| `apps/desktop/src` | 界面、交互与 IPC 调用。业务计算不写在这里 |
| `crates/rrss-desktop` | Tauri 窗口与命令适配；命令只转接 `rrss-app` |
| `crates/rrss-app` | 业务编排：查询、动作、后台作业、调度、Pi 运行时 |
| `crates/rrss-core` | SQLite、领域模型、抓取、规则、排序、聚类、网络与平台适配 |
| `crates/rrss-render` | HTML / Markdown 清洗并转成块 IR |
| `crates/rrss-ai` | HTTP 补全、嵌入与 Pi 协议 |
| `pi/` | 随应用发布的 Pi 系统提示、扩展与技能 |
| `testdata/` | 馈源、渲染、TLS 与 Pi 模拟数据 |

## 约定

这些约定保护数据、安全和跨平台行为，提交前请对照检查：

- **IPC**：前端只经 `apps/desktop/src/lib/api.ts` 调用宿主。`apps/desktop/src/bindings.ts` 由 `crates/rrss-app/src/views.rs` 生成，改字段后运行 `cargo test -p rrss-app --lib export_ts`，不要手改绑定。
- **数据库**：表结构改动同时改 `crates/rrss-core/src/schema.sql` 与 `crates/rrss-core/src/db/migrations.rs`。数据库访问走宿主命令的 `state::with_db` 或后台作业的 `jobs::spawn_db`，不在界面线程开库。
- **内容安全**：正文与模型输出只渲染清洗后的块 IR；不使用 `dangerouslySetInnerHTML` 和 iframe，主界面不加载远程脚本与远程媒体。外部网页只在隔离的子 webview 里打开，该 webview 不授予 IPC。
- **平台差异**：Windows / Linux 的差异只写在 `rrss-core::platform`；需要原生窗口或 webview 句柄的代码（WebView2 / WebKitGTK、Windows 窗口图标）集中在 `rrss-desktop` 的 `webview_native`。业务代码不写 `cfg(windows)` / `cfg(unix)`。
- **网络**：新的网络请求必须接入对应的 `NetworkScope`，不要直接创建未配置代理的 `reqwest::Client`。Windows 的系统代理从 `rrss-core::platform` 读取。
- **隐私**：界面错误、日志与诊断里的密钥、令牌和代理认证统一经 `rrss-core::logsafe` 清洗；正文、查询与模型响应不写进日志。
- **演示数据**：截图、预览与测试夹具用虚构的来源、作者和 `.example` 域名，不把编造的内容挂在真实媒体、作者或项目名下，不写真实的内网地址。
- **展示文案**：界面目前只有中文，多语言正在分批推进。后端不拼给用户看的句子：原因、状态、校验与错误提示由 `views.rs` 返回「码 + 参数」，由前端格式化；落库存码不存句。前端文案写进 `apps/desktop/src/locales/zh-CN/<功能>.json`，用 `src/i18n` 导出的 `useTranslation` / `Trans` 取，带数量的句子用 `count` 与 `_other` 后缀的键，不在代码里拼；日期与数字用 `Intl` 时语言取 `locale()`。还没迁移的旧文件里的中文字面量由 `check:i18n` 按文件计数，不许超过基线；减少时只提示，一批迁移合并后运行 `node tooling/check-i18n.mjs --update` 收紧基线。测试按角色或码断言，不新增对中文文案的断言。
- **样式**：使用 `apps/desktop/src/styles/tokens.css` 的语义令牌与 4px 网格；长列表用虚拟化。

## 测试

提交前至少跑与改动相关的部分：

```sh
pnpm --dir apps/desktop check:frontend   # 类型、Hooks、依赖与 IPC 契约检查
pnpm --dir apps/desktop test             # 前端单测
pnpm --dir apps/desktop test:affected    # 按改动挑选单测、e2e 与 Rust 测试
cargo test --locked --workspace --exclude rrss-desktop
cargo check --locked -p rrss-desktop   # 桌面 crate 没有单测，只做编译检查
```

- e2e 用模拟宿主跑在 Chromium 上：首次先运行 `pnpm --dir apps/desktop exec playwright install chromium`，之后用 `pnpm --dir apps/desktop test:e2e`。
- 自动测试使用一次性数据库，不调用模型、不下载；不要让测试碰日常库。
- 规则、存储、迁移、代理与脱敏相关的改动要有测试；修缺陷时先写能复现它的回归测试。
- Windows 目标的编译由 CI 的 `windows-check` 负责。
- 格式化只作用于改动的文件：Rust 用 `rustfmt --edition 2024 <文件>`，不要对整个 crate 重排。

## 提交

- 一个 PR 做一件事，说明改了什么行为、怎么验证的。
- 用户能感知的变化（新功能、行为变化、修复）在 [CHANGELOG.md](CHANGELOG.md) 的「未发布」下加一条，写给用户看，不写实现细节；纯重构、测试与文档改动不用写。
- 界面有明显变化时附截图。README 的演示截图可用 `RRSS_README_SHOTS=1 pnpm --dir apps/desktop exec playwright test e2e/readme-shots.spec.ts` 重新生成。
- 安全问题不要开公开 issue，按 [SECURITY.md](SECURITY.md) 私下报告。
- 这个仓库由上游开发仓库同步生成，上游每合并一项改动，这里就多一个对应的提交，提交说明写给外部读者。PR 不在这里直接合并：评审通过后，维护者把改动并入上游，同步时作为单独的提交出现在这里，PR 会附说明后关闭，提交里用 `Co-authored-by` 署上你的名字。
- 提交的代码按仓库的 [MIT 许可](LICENSE) 发布。
