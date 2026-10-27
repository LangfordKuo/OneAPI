use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

use serde::Serialize;

use crate::models::now_ms;

const WINDOW_SECONDS: usize = 10;

#[derive(Debug, Default)]
pub struct LiveMetrics {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    global: Window,
    channels: HashMap<i64, Window>,
}

#[derive(Debug)]
struct Window {
    buckets: Vec<u64>,
    index: usize,
    last_second: i64,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            buckets: vec![0; WINDOW_SECONDS],
            index: 0,
            last_second: 0,
        }
    }
}

impl Window {
    fn refresh(&mut self, second: i64) {
        if second < self.last_second {
            self.buckets.fill(0);
            self.index = 0;
            self.last_second = second;
            return;
        }

        let steps = (second - self.last_second).min(WINDOW_SECONDS as i64) as usize;
        for _ in 0..steps {
            self.index = (self.index + 1) % WINDOW_SECONDS;
            self.buckets[self.index] = 0;
        }
        self.last_second = second;
    }

    fn add(&mut self, second: i64, tokens: u64) {
        self.refresh(second);
        self.buckets[self.index] = self.buckets[self.index].saturating_add(tokens);
    }

    fn rate(&mut self, second: i64) -> f64 {
        self.refresh(second);
        self.buckets.iter().sum::<u64>() as f64 / WINDOW_SECONDS as f64
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
        let mut inner = self.lock();
        inner.global.add(second, tokens);
        if let Some(channel_id) = channel_id {
            inner
                .channels
                .entry(channel_id)
                .or_default()
                .add(second, tokens);
        }
    }

    pub fn snapshot(&self) -> LiveSnapshot {
        let second = now_ms() / 1000;
        let mut inner = self.lock();

        let mut channels = Vec::new();
        let mut stale = Vec::new();
        for (channel_id, window) in inner.channels.iter_mut() {
            let rate = window.rate(second);
            if rate > 0.0 {
                channels.push(ChannelRate {
                    channel_id: *channel_id,
                    tok_s: round_one(rate),
                });
            } else {
                stale.push(*channel_id);
            }
        }
        for channel_id in stale {
            inner.channels.remove(&channel_id);
        }
        channels.sort_by(|left, right| {
            right
                .tok_s
                .partial_cmp(&left.tok_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        LiveSnapshot {
            timestamp: now_ms(),
            global_tok_s: round_one(inner.global.rate(second)),
            channels,
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
