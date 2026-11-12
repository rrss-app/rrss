//! Application-owned admission, acquired anew for each real HTTP attempt.
use std::{future::Future, pin::Pin};

pub type Permit = Box<dyn Send>;
pub type Acquisition<'a> = Pin<Box<dyn Future<Output = crate::Result<Permit>> + Send + 'a>>;
pub type Response<'a> = Pin<Box<dyn Future<Output = crate::Result<()>> + Send + 'a>>;

pub trait Admission: Send + Sync {
    fn acquire(&self) -> Acquisition<'_>;

    /// Observe headers while the request permit is still held, before reading
    /// an error body. The application may persist service-wide Retry-After.
    fn response(&self, _status: u16, _retry_after: Option<String>) -> Response<'_> {
        Box::pin(async { Ok(()) })
    }
}
