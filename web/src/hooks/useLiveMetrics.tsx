import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react';
import { readSse } from '@/api/sse';

export interface LiveChannelRate {
  channel_id: number;
  tok_s: number;
}

export interface LiveMetricsSnapshot {
  timestamp: number;
  global_tok_s: number;
  channels: LiveChannelRate[];
  /**
   * Monotonic count of usage rows committed by the server process. A change
   * means new log rows exist; older servers omit it, so it is optional.
   */
  usage_total?: number;
}

interface LiveState {
  snapshot: LiveMetricsSnapshot;
  /** Increments whenever `usage_total` advances between samples. */
  usageSeq: number;
  /** Last observed usage counter, used to detect advances. */
  usageTotal: number;
  /** False while the SSE connection is down; pages then fall back to polling. */
  connected: boolean;
}

const EMPTY_SNAPSHOT: LiveMetricsSnapshot = {
  timestamp: 0,
  global_tok_s: 0,
  channels: [],
};

const EMPTY: LiveState = {
  snapshot: EMPTY_SNAPSHOT,
  usageSeq: 0,
  usageTotal: 0,
  connected: false,
};

const RECONNECT_DELAY_MS = 3000;

const LiveMetricsContext = createContext<LiveState>(EMPTY);

/**
 * Subscribes once to /api/metrics/stream. Unlike EventSource this can observe
 * the HTTP status, so an expired session stops reconnecting instead of
 * hammering the endpoint forever.
 *
 * The stream emits one JSON sample per second that includes both the
 * throughput rates and a monotonic `usage_total`. Consumers watch `usageSeq`
 * (derived from `usage_total` advances) as a cheap change signal to refetch
 * their queries. Frames without the counter still work, so older servers keep
 * the pages functional through the polling fallback.
 */
function useLiveStream(): LiveState {
  const [state, setState] = useState<LiveState>(EMPTY);

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

        setState((previous) =>
          previous.connected ? previous : { ...previous, connected: true },
        );

        for await (const event of readSse(response)) {
          try {
            const snapshot = JSON.parse(event.data) as LiveMetricsSnapshot;
            setState((previous) => {
              const usageTotal = snapshot.usage_total;
              const firstFrame = previous.snapshot.timestamp === 0;
              const advanced =
                !firstFrame &&
                typeof usageTotal === 'number' &&
                usageTotal !== previous.usageTotal;
              return {
                snapshot,
                usageTotal:
                  typeof usageTotal === 'number'
                    ? usageTotal
                    : previous.usageTotal,
                usageSeq: advanced ? previous.usageSeq + 1 : previous.usageSeq,
                connected: true,
              };
            });
          } catch {
            // Ignore malformed frames; the next tick will replace them.
          }
        }
      } catch {
        if (controller.signal.aborted) {
          return;
        }
      }

      setState((previous) =>
        previous.connected ? { ...previous, connected: false } : previous,
      );

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

  return state;
}

export function LiveMetricsProvider({ children }: { children: ReactNode }) {
  const state = useLiveStream();
  return (
    <LiveMetricsContext.Provider value={state}>
      {children}
    </LiveMetricsContext.Provider>
  );
}

/** Live throughput snapshot, sampled once per second. */
export function useLiveMetrics(): LiveMetricsSnapshot {
  return useContext(LiveMetricsContext).snapshot;
}

/** Monotonic counter that changes when new usage logs are persisted. */
export function useLiveUsageSignal(): number {
  return useContext(LiveMetricsContext).usageSeq;
}

/** Whether the live SSE stream is currently connected. */
export function useLiveConnected(): boolean {
  return useContext(LiveMetricsContext).connected;
}
