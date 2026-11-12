use std::{
    io,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) struct Capture {
    pub dispatch: tracing::Dispatch,
    buffer: Buffer,
}
impl Capture {
    pub fn new() -> Self {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish();
        Self {
            dispatch: tracing::Dispatch::new(subscriber),
            buffer,
        }
    }
    pub fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        tracing::dispatcher::with_default(&self.dispatch, f)
    }
    pub fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.buffer.0.lock().unwrap())).unwrap()
    }
}
