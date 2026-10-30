import { useEffect, useState } from 'react';

/**
 * Return `value` after it has stayed unchanged for `delayMs`.
 *
 * Used by the log filters: the input stays responsive (it is bound to the raw
 * value) while the server query only re-runs once typing pauses.
 */
export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delayMs);
    return () => window.clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}
