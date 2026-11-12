//! Pi RPC：`protocol` 负责命令构造与事件解码，`rpc` 负责请求 id 关联与超时，都不依赖具体进程。
//! 日志 target：`rrss_ai::pi`。

pub mod protocol;
pub mod rpc;

pub use protocol::*;
pub use rpc::RpcCorrelator;
