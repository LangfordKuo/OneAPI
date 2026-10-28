import { useSyncExternalStore } from 'react';

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
    }, 30_000);
  }

  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      window.clearInterval(timer);
      timer = null;
    }
  };
}

function getSnapshot() {
  return snapshot;
}

/** Shared clock for time-dependent UI; re-renders subscribers every 30s. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
