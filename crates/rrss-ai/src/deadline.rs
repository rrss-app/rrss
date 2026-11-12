//! Deadlines belong to operations, not shared proxy clients.
use crate::{Error, Result};
use std::{future::Future, time::Duration};
use tokio::time::Instant;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
pub const ITEM_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn timeout_error() -> Error {
    Error::Request {
        timeout: true,
        message: "模型请求超时",
    }
}

pub async fn until<T>(deadline: Instant, future: impl Future<Output = Result<T>>) -> Result<T> {
    // Do not poll a new HTTP attempt after the original operation expired.
    if Instant::now() >= deadline {
        return Err(timeout_error());
    }
    tokio::time::timeout_at(deadline, future)
        .await
        .map_err(|_| timeout_error())?
}
