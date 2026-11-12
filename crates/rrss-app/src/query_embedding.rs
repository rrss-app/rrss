//! 对话与 Pi 工具共用的查询嵌入；不计预算或用量，不写文章向量。

use rrss_ai::embed::{EmbedBatchRequest, embed_batch_until};
use rrss_core::{ai_store, vector_space};
use rusqlite::Connection;

pub(crate) struct QueryEmbedding {
    space_id: i64,
    endpoint: ai_store::EmbedEndpoint,
}

impl QueryEmbedding {
    pub(crate) fn load(conn: &Connection) -> rrss_core::Result<Option<Self>> {
        let Some(space) = vector_space::active(conn)? else {
            return Ok(None);
        };
        if ai_store::resolve_embed(conn)?.is_none() {
            return Ok(None);
        }
        // 设置可能已指向重建目标；查询必须继续使用 active 空间自己的端点。
        let Some(endpoint) = vector_space::endpoint(conn, &space)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            space_id: space.id,
            endpoint,
        }))
    }

    pub(crate) async fn embed_until(
        &self,
        client: &reqwest::Client,
        query: &str,
        deadline: tokio::time::Instant,
    ) -> anyhow::Result<Vec<f32>> {
        let out = embed_batch_until(
            client,
            &EmbedBatchRequest {
                base_url: self.endpoint.base_url.clone(),
                api_key: self.endpoint.api_key.clone(),
                model: self.endpoint.model.clone(),
                inputs: vec![query.to_owned()],
            },
            deadline,
        )
        .await?;
        out.vectors
            .into_iter()
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| rrss_ai::Error::EmptyEmbedding.into())
    }

    /// 请求期间可能切换空间；用查询时仍生效的空间核对指纹和截断维度。
    pub(crate) fn fit(
        &self,
        conn: &Connection,
        vector: Vec<f32>,
    ) -> rrss_core::Result<Option<Vec<f32>>> {
        Ok(vector_space::active(conn)?
            .filter(|space| {
                space.id == self.space_id
                    && space.model == self.endpoint.model
                    && space.base_url == self.endpoint.base_url
            })
            .map(|space| vector_space::fit(vector, space.dims as usize)))
    }
}
