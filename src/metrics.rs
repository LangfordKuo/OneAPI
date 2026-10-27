use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use serde::Serialize;

use crate::models::now_ms;

const WINDOW_SECONDS: usize = 10;

#[derive(Debug, Default)]
pub struct LiveMetrics {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    global: Arc<Window>,
    channels: HashMap<i64, Arc<Window>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            global: Arc::new(Window::default()),
            channels: HashMap::new(),
        }
    }
}

/// Per-second buckets. Token increments are atomic so the hot path does not
/// contend on the outer map lock; only bucket rollover takes a short lock.
#[derive(Debug)]
struct Window {
    buckets: [AtomicU64; WINDOW_SECONDS],
    index: AtomicUsize,
    last_second: AtomicI64,
    rollover: Mutex<()>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            index: AtomicUsize::new(0),
            last_second: AtomicI64::new(0),
            rollover: Mutex::new(()),
        }
    }
}

impl Window {
    fn rollover_lock(&self) -> MutexGuard<'_, ()> {
        self.rollover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn refresh_locked(&self, second: i64) {
        let last = self.last_second.load(Ordering::Relaxed);
        if second < last {
            for bucket in &self.buckets {
                bucket.store(0, Ordering::Relaxed);
            }
            self.index.store(0, Ordering::Relaxed);
            self.last_second.store(second, Ordering::Relaxed);
            return;
        }

        let steps = (second - last).min(WINDOW_SECONDS as i64) as usize;
        let mut index = self.index.load(Ordering::Relaxed);
        for _ in 0..steps {
            index = (index + 1) % WINDOW_SECONDS;
            self.buckets[index].store(0, Ordering::Relaxed);
        }
        self.index.store(index, Ordering::Relaxed);
        self.last_second.store(second, Ordering::Relaxed);
    }

    fn add(&self, second: i64, tokens: u64) {
        if self.last_second.load(Ordering::Relaxed) != second {
            let _guard = self.rollover_lock();
            if self.last_second.load(Ordering::Relaxed) != second {
                self.refresh_locked(second);
            }
        }

        let index = self.index.load(Ordering::Relaxed) % WINDOW_SECONDS;
        self.buckets[index].fetch_add(tokens, Ordering::Relaxed);
    }

    fn rate(&self, second: i64) -> f64 {
        let _guard = self.rollover_lock();
        self.refresh_locked(second);
        let total: u64 = self
            .buckets
            .iter()
            .map(|bucket| bucket.load(Ordering::Relaxed))
            .sum();
        total as f64 / WINDOW_SECONDS as f64
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelRate {
    pub channel_id: i64,
    pub tok_s: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveSnapshot {
    pub timestamp: i64,
    pub global_tok_s: f64,
    pub channels: Vec<ChannelRate>,
}

impl LiveMetrics {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn add_tokens(&self, channel_id: Option<i64>, tokens: u64) {
        if tokens == 0 {
            return;
        }
        self.add_tokens_at(channel_id, tokens, now_ms() / 1000);
    }

    fn add_tokens_at(&self, channel_id: Option<i64>, tokens: u64, second: i64) {
        let (global, channel) = {
            let mut inner = self.lock();
            let global = inner.global.clone();
            let channel = channel_id.map(|id| inner.channels.entry(id).or_default().clone());
            (global, channel)
        };

        global.add(second, tokens);
        if let Some(window) = channel {
            window.add(second, tokens);
        }
    }

    pub fn snapshot(&self) -> LiveSnapshot {
        let second = now_ms() / 1000;
        let (global, channels) = {
            let inner = self.lock();
            let global = inner.global.clone();
            let channels: Vec<(i64, Arc<Window>)> = inner
                .channels
                .iter()
                .map(|(id, window)| (*id, window.clone()))
                .collect();
            (global, channels)
        };

        let mut rates = Vec::new();
        let mut stale = Vec::new();
        for (channel_id, window) in channels {
            let rate = window.rate(second);
            if rate > 0.0 {
                rates.push(ChannelRate {
                    channel_id,
                    tok_s: round_one(rate),
                });
            } else {
                stale.push(channel_id);
            }
        }
        rates.sort_by(|left, right| {
            right
                .tok_s
                .partial_cmp(&left.tok_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if !stale.is_empty() {
            let mut inner = self.lock();
            for channel_id in stale {
                inner.channels.remove(&channel_id);
            }
        }

        LiveSnapshot {
            timestamp: now_ms(),
            global_tok_s: round_one(global.rate(second)),
            channels: rates,
        }
    }
}

fn round_one(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_recent_channel_and_global_rates() {
        let metrics = LiveMetrics::default();
        let now = now_ms() / 1000;

        metrics.add_tokens_at(Some(7), 100, now);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.global_tok_s, 10.0);
        assert_eq!(snapshot.channels.len(), 1);
        assert_eq!(snapshot.channels[0].channel_id, 7);
        assert_eq!(snapshot.channels[0].tok_s, 10.0);
    }

    #[test]
    fn old_tokens_leave_the_window() {
        let metrics = LiveMetrics::default();
        let now = now_ms() / 1000;

        metrics.add_tokens_at(Some(1), 100, now - (WINDOW_SECONDS as i64 + 5));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.global_tok_s, 0.0);
        assert!(snapshot.channels.is_empty());
    }
}
