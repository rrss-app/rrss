# Pi RPC 测试数据

`digest_rpc.py` 是确定性的模拟 Pi 0.84 进程，仅由 `python3 scripts/dev-smoke.py --digest` 复制到一次性测试目录并运行。

- 模拟新建/恢复会话、提示词预检拒绝、工具循环和 `agent_settled`。
- 通过真实 `rrss tool get_digest --id` / `get_article` 读取隔离库，检查第二条来源的文章 ID。
- 从不调用模型端点；测试成功不代表真实模型的回答质量已验证。
- 不作为日常 Pi 安装，不写入用户数据库或用户会话。

`scenario_rpc.py` 是 `rrss-app` L2 集成测试（`crates/rrss-app/tests/pi_runtime.rs`）用的脚本化假 Pi：提示词里的 `[[model-error]]`、`[[exit]]`、`[[slow]]`、`[[confirm:毫秒]]`、`[[select]]`、`[[reject]]` 等标记选择场景，收到的每条命令记到会话目录旁的 `fake-pi.log`，每次启动先记一行 `START`（参数与 RRSS 设置的环境变量）。同样不调用模型、不联网。

三个假 Pi 都回应启动握手：`get_state` 报告 `--model` 里的模型，并回应 `get_available_thinking_levels`。用它们的测试要先在库里配好对话连接，否则发送前就报配置错误。

`fixtures/` 是 `rrss-ai` L1 协议契约测试读取的 JSONL 样本，说明见其中的 README。
