use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic count of usage log rows committed by this process.
///
/// The admin SSE stream samples this once per second and folds it into the
/// existing snapshot, so clients can detect "new log rows exist" without a
/// second endpoint or a payload change that older frontends cannot parse.
#[derive(Debug, Default)]
pub struct EventBus {
    usage_rows: AtomicU64,
}

impl EventBus {
    pub fn usage_written(&self, count: usize) {
        if count > 0 {
            self.usage_rows.fetch_add(count as u64, Ordering::Relaxed);
        }
    }

    pub fn usage_rows(&self) -> u64 {
        self.usage_rows.load(Ordering::Relaxed)
    }
}
