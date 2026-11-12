# Pi RPC 协议样本

`rrss-ai` 的 L1 协议契约测试逐行解码这里的 JSONL，断言对话需要的事件与字段（测试分层见 Pi 运行时规格）。

- `docs-0.87/`：按 Pi 0.84.4 / 0.87.1 npm 包内 `docs/json.md`、`docs/rpc-commands.md`、`docs/rpc-extension-ui.md` 的示例手写，覆盖纯文本、工具循环、模型错误、自动重试最终失败、压缩、确认请求与中止。不是真实录制。
- `<version>/`：`scripts/pi-compat.mjs` 用真实 Pi 加本地模拟模型录制，另有 `REPORT.md`（与上一 tested 版本的差异、会话恢复结果）。本机路径已替换成 `<work>` / `<repo>`。压缩不在录制范围内，只由手写样本覆盖。
- 两类目录跑同一组断言；某个目录没有的场景跳过。
