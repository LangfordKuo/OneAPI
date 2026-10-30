import { useSyncExternalStore } from 'react';

interface Clock {
  subscribe: (listener: () => void) => () => void;
  getSnapshot: () => number;
}

/**
 * Shared clock. The snapshot only changes inside the timer, so components read
 * a stable value during render instead of calling `Date.now()` themselves.
 */
function createClock(intervalMs: number): Clock {
  let snapshot = Date.now();
  let timer: number | null = null;
  const listeners = new Set<() => void>();

  function subscribe(listener: () => void) {
    snapshot = Date.now();
    listeners.add(listener);

    if (timer === null) {
      timer = window.setInterval(() => {
        snapshot = Date.now();
        for (const notify of listeners) {
          notify();
        }
      }, intervalMs);
    }

    return () => {
      listeners.delete(listener);
      if (listeners.size === 0 && timer !== null) {
        window.clearInterval(timer);
        timer = null;
      }
    };
  }

  return { subscribe, getSnapshot: () => snapshot };
}

const minuteClock = createClock(30_000);
const secondClock = createClock(1_000);

/** Shared clock for time-dependent UI; re-renders subscribers every 30s. */
export function useNow(): number {
  return useSyncExternalStore(
    minuteClock.subscribe,
    minuteClock.getSnapshot,
    minuteClock.getSnapshot,
  );
}

/** Per-second clock for counters that must tick visibly (e.g. uptime). */
export function useNowSeconds(): number {
  return useSyncExternalStore(
    secondClock.subscribe,
    secondClock.getSnapshot,
    secondClock.getSnapshot,
  );
}
