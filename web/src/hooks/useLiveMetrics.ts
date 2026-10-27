import { useEffect, useState } from 'react';

export interface LiveChannelRate {
  channel_id: number;
  tok_s: number;
}

export interface LiveMetricsSnapshot {
  timestamp: number;
  global_tok_s: number;
  channels: LiveChannelRate[];
}

const EMPTY: LiveMetricsSnapshot = {
  timestamp: 0,
  global_tok_s: 0,
  channels: [],
};

/**
 * Subscribes to /api/metrics/stream. EventSource reconnects automatically and
 * keeps the page updated without a reload.
 */
export function useLiveMetrics(): LiveMetricsSnapshot {
  const [snapshot, setSnapshot] = useState<LiveMetricsSnapshot>(EMPTY);

  useEffect(() => {
    const source = new EventSource('/api/metrics/stream');
    source.onmessage = (event) => {
      try {
        setSnapshot(JSON.parse(event.data) as LiveMetricsSnapshot);
      } catch {
        // Ignore malformed frames; the next tick will replace them.
      }
    };
    return () => source.close();
  }, []);

  return snapshot;
}
