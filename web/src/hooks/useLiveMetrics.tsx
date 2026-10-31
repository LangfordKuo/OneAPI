import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react';
import { readSse } from '@/api/sse';
import { handleUnauthorized } from '@/api/client';

export interface LiveChannelRate {
  channel_id: number;
  tok_s: number;
}

export interface LiveMetricsSnapshot {
  timestamp: number;
  global_tok_s: number;
  /**
   * Gateway requests currently producing output (streams plus in-flight
   * non-streaming requests). Older servers omit it.
   */
  active_sessions?: number;
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
  active_sessions: 0,
  channels: [],
};

const EMPTY: LiveState = {
  snapshot: EMPTY_SNAPSHOT,
  usageSeq: 0,
  usageTotal: 0,
  connected: false,
};

const RECONNECT_DELAY_MS = 3000;

// One context per slice: a single context object would give every consumer a
// new value on every one-second sample, re-rendering pages (for example the log
// table) that only care about the rare usage/connection signals.
const SnapshotContext = createContext<LiveMetricsSnapshot>(EMPTY.snapshot);
const UsageSeqContext = createContext<number>(EMPTY.usageSeq);
const ConnectedContext = createContext<boolean>(EMPTY.connected);

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
          // The session expired: stop reconnecting and hand the 401 to the
          // shared handler so the app invalidates `['me']` and shows login.
          // The provider only mounts for an authenticated admin, so a
          // successful login remounts it and restarts this stream.
          stopped = true;
          setState((previous) =>
            previous.connected ? { ...previous, connected: false } : previous,
          );
          handleUnauthorized();
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
    <UsageSeqContext.Provider value={state.usageSeq}>
      <ConnectedContext.Provider value={state.connected}>
        <SnapshotContext.Provider value={state.snapshot}>
          {children}
        </SnapshotContext.Provider>
      </ConnectedContext.Provider>
    </UsageSeqContext.Provider>
  );
}

/** Live throughput snapshot, sampled once per second. */
export function useLiveMetrics(): LiveMetricsSnapshot {
  return useContext(SnapshotContext);
}

/** Monotonic counter that changes when new usage logs are persisted. */
export function useLiveUsageSignal(): number {
  return useContext(UsageSeqContext);
}

/** Whether the live SSE stream is currently connected. */
export function useLiveConnected(): boolean {
  return useContext(ConnectedContext);
}
