use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use serde::Serialize;

use crate::models::now_ms;

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

/// Instantaneous rate with one-second buckets. `rate(second)` reports the tokens
/// counted in the most recently completed second (second - 1), without any
/// sliding-window smoothing.
#[derive(Debug, Default)]
struct Window {
    bucket: Mutex<Bucket>,
}

#[derive(Debug, Default)]
struct Bucket {
    current_second: i64,
    current_tokens: u64,
    previous_second: i64,
    previous_tokens: u64,
}

impl Bucket {
    /// Move the cursor to `second`, keeping the bucket that just completed.
    /// Gaps reset the previous bucket because nothing was generated meanwhile.
    fn advance(&mut self, second: i64) {
        if second <= self.current_second {
            return;
        }

        if second == self.current_second + 1 && self.current_second != 0 {
            self.previous_second = self.current_second;
            self.previous_tokens = self.current_tokens;
        } else {
            self.previous_second = second - 1;
            self.previous_tokens = 0;
        }

        self.current_second = second;
        self.current_tokens = 0;
    }
}

impl Window {
    fn lock(&self) -> MutexGuard<'_, Bucket> {
        self.bucket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn add(&self, second: i64, tokens: u64) {
        let mut bucket = self.lock();
        if second < bucket.current_second {
            // A concurrent caller may still be in the second that just rolled
            // over; keep those tokens attached to the completed bucket.
            if second == bucket.previous_second {
                bucket.previous_tokens = bucket.previous_tokens.saturating_add(tokens);
            }
            return;
        }

        bucket.advance(second);
        bucket.current_tokens = bucket.current_tokens.saturating_add(tokens);
    }

    fn rate(&self, second: i64) -> f64 {
        let mut bucket = self.lock();
        bucket.advance(second);

        if bucket.previous_second == second - 1 {
            bucket.previous_tokens as f64
        } else {
            0.0
        }
    }

    /// A channel is idle only when neither the completed nor the active bucket
    /// holds tokens. The active bucket matters because a stream that just
    /// started has not completed its first second yet.
    fn is_idle(&self) -> bool {
        let bucket = self.lock();
        bucket.previous_tokens == 0 && bucket.current_tokens == 0
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
        let mut snapshot = self.snapshot_at(now_ms() / 1000);
        snapshot.timestamp = now_ms();
        snapshot
    }

    fn snapshot_at(&self, second: i64) -> LiveSnapshot {
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
            } else if window.is_idle() {
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
            timestamp: second * 1000,
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
    fn reports_the_last_completed_second() {
        let metrics = LiveMetrics::default();

        metrics.add_tokens_at(Some(7), 100, 1_000);
        // Tokens from the active second are not part of the rate yet.
        assert_eq!(metrics.snapshot_at(1_000).global_tok_s, 0.0);

        let snapshot = metrics.snapshot_at(1_001);
        assert_eq!(snapshot.global_tok_s, 100.0);
        assert_eq!(snapshot.channels.len(), 1);
        assert_eq!(snapshot.channels[0].channel_id, 7);
        assert_eq!(snapshot.channels[0].tok_s, 100.0);

        metrics.add_tokens_at(Some(7), 40, 1_001);
        assert_eq!(metrics.snapshot_at(1_001).global_tok_s, 100.0);
        assert_eq!(metrics.snapshot_at(1_002).global_tok_s, 40.0);
    }

    #[test]
    fn drops_to_zero_after_an_idle_second() {
        let metrics = LiveMetrics::default();

        metrics.add_tokens_at(Some(1), 100, 1_000);
        assert_eq!(metrics.snapshot_at(1_001).global_tok_s, 100.0);

        let snapshot = metrics.snapshot_at(1_003);
        assert_eq!(snapshot.global_tok_s, 0.0);
        assert!(snapshot.channels.is_empty());
    }

    #[test]
    fn late_tokens_stay_in_the_completed_second() {
        let metrics = LiveMetrics::default();

        metrics.add_tokens_at(Some(2), 30, 1_000);
        metrics.add_tokens_at(Some(2), 70, 1_001);
        // A caller that read the clock before rollover still lands in 1_000.
        metrics.add_tokens_at(Some(2), 20, 1_000);

        assert_eq!(metrics.snapshot_at(1_001).global_tok_s, 50.0);
    }
}
