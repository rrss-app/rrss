# Pi 0.87.1 兼容核对

对比上一 tested 版本 0.84.4。

CLI 参数取自 `pi --help`；RPC 命令、事件类型、设置键和环境变量取自发布包里的文档，文档改版会造成误报。判断事件是否还在，以下面「录制中出现的事件」和 L1 / L3 测试为准。

## CLI 参数

无变化。

## RPC 命令

- **移除（可能破坏）**：`agent_end`、`agent_settled`、`agent_start`、`bash_execution_update`、`extension_error`、`queue_update`

## 事件类型

- **移除（可能破坏）**：`queue_update`、`text_end`、`text_start`、`thinking`、`toolcall_start`

## 设置键

- **移除（可能破坏）**：`trackingId`
- 新增：`cacheWarming`、`compaction.modelOverrides`、`retry.maxAgentDelayMs`、`terminal.showTerminalProgress`

## 环境变量

- 新增：`PI_RADIUS_GATEWAY`

## 录制

场景：text、tool_loop、model_error、auto_retry、confirm、abort。本地模拟模型，见 `scripts/pi-compat.mjs`。压缩不在录制范围内，仍由 `docs-0.87/` 的手写样本覆盖。

## 录制中出现的事件

`agent_end`、`agent_settled`、`agent_start`、`auto_retry_end`、`auto_retry_start`、`entry_appended`、`extension_ui_request`、`message_end`、`message_start`、`message_update/text_delta`、`message_update/text_end`、`message_update/text_start`、`message_update/toolcall_delta`、`message_update/toolcall_end`、`message_update/toolcall_start`、`response`、`tool_execution_end`、`tool_execution_start`、`turn_end`、`turn_start`

## 会话恢复

0.84.4 创建的会话：切换成功；新一轮请求带着旧上下文。
