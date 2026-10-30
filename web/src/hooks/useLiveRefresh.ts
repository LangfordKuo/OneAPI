import { useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useRef } from 'react';
import { useLiveConnected, useLiveUsageSignal } from './useLiveMetrics';

/**
 * Refetches the given query-key prefixes when new usage logs are persisted.
 *
 * Server events are coalesced: at most one refetch fires per `minIntervalMs`,
 * so a burst of gateway traffic costs a bounded number of aggregate queries
 * instead of one per request. The first event after an idle period refreshes
 * immediately. Hidden tabs skip the refetches and catch up on visibility.
 */
export function useLiveRefresh(
  queryKeys: readonly (readonly unknown[])[],
  minIntervalMs = 3000,
): void {
  const queryClient = useQueryClient();
  const sequence = useLiveUsageSignal();

  // Key arrays are rebuilt on every render; keep the latest value in a ref so
  // the effects only react to actual server signals. Refs are written after
  // commit because they must not be touched during render.
  const keysRef = useRef(queryKeys);
  const latestRef = useRef(sequence);
  const handledRef = useRef(sequence);
  const flushedAtRef = useRef(0);

  useEffect(() => {
    keysRef.current = queryKeys;
  }, [queryKeys]);

  useEffect(() => {
    latestRef.current = sequence;
  }, [sequence]);

  const flush = useCallback(() => {
    if (document.hidden) {
      return;
    }
    handledRef.current = latestRef.current;
    flushedAtRef.current = Date.now();
    for (const queryKey of keysRef.current) {
      void queryClient.invalidateQueries({ queryKey });
    }
  }, [queryClient]);

  // A background tab does not burn database time on every gateway request;
  // the first render after it becomes visible catches up once.
  useEffect(() => {
    const onVisibilityChange = () => {
      if (
        document.visibilityState === 'visible' &&
        latestRef.current !== handledRef.current
      ) {
        flush();
      }
    };
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () =>
      document.removeEventListener('visibilitychange', onVisibilityChange);
  }, [flush]);

  useEffect(() => {
    if (sequence === handledRef.current || document.hidden) {
      return;
    }

    // Align bursts to a fixed cadence counted from the last flush, so a steady
    // stream of events cannot postpone the refresh indefinitely.
    const delay = Math.max(0, minIntervalMs - (Date.now() - flushedAtRef.current));
    const timer = window.setTimeout(flush, delay);
    return () => window.clearTimeout(timer);
  }, [sequence, minIntervalMs, flush]);
}

/**
 * Polling interval for live pages. The SSE stream drives refreshes, so this is
 * only a safety net while connected and the real update path when it is down.
 */
export function useLivePollingInterval(
  connectedIntervalMs = 30_000,
  fallbackIntervalMs = 5_000,
): number {
  return useLiveConnected() ? connectedIntervalMs : fallbackIntervalMs;
}
