//! 补全客户端 + Pi RPC + 作业队列。不是第二套 agent 内核。
//!
//! 设计见 AI、Pi 与记忆规格。

pub mod admission;
pub mod complete;
pub mod deadline;
pub mod embed;
pub mod models;
pub mod pi;

pub use complete::probe::{CapabilityProbe, Support, probe_capabilities};
pub use complete::{
    CompleteRequest, CompleteResponse, Error, Protocol, Result, complete,
    complete_cached, digest_json_schema, digest_pick_json_schema, facts_json_schema, http_client,
    segment_json_schema, strip_thinking, summary_json_schema, translate_json_schema,
};
pub use embed::{
    EmbedBatchRequest, EmbedBatchResponse, EmbedRequest, EmbedResponse, embed, embed_batch,
    retry_smaller,
};
pub use models::list_models;
