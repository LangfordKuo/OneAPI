import { useEffect, useState } from 'react';
import { readSse } from '../api/sse';

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

const RECONNECT_DELAY_MS = 3000;

/**
 * Subscribes to /api/metrics/stream. Unlike EventSource this can observe the
 * HTTP status, so an expired session stops reconnecting instead of hammering
 * the endpoint forever.
 */
export function useLiveMetrics(): LiveMetricsSnapshot {
  const [snapshot, setSnapshot] = useState<LiveMetricsSnapshot>(EMPTY);

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | undefined;
    let stopped = false;

    const connect = async () => {
      try {
        const response = await fetch('/api/metrics/stream', {
          credentials: 'include',
          headers: { Accept: 'text/event-stream' },
          signal: controller.signal,
        });

        if (response.status === 401) {
          stopped = true;
          return;
        }
        if (!response.ok || !response.body) {
          throw new Error(`metrics stream failed with ${response.status}`);
        }

        for await (const event of readSse(response)) {
          try {
            setSnapshot(JSON.parse(event.data) as LiveMetricsSnapshot);
          } catch {
            // Ignore malformed frames; the next tick will replace them.
          }
        }
      } catch {
        if (controller.signal.aborted) {
          return;
        }
      }

      if (!stopped && !controller.signal.aborted) {
        timer = window.setTimeout(() => void connect(), RECONNECT_DELAY_MS);
      }
    };

    void connect();

    return () => {
      controller.abort();
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
    };
  }, []);

  return snapshot;
}
